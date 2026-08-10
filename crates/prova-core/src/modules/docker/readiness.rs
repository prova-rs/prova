//! Container readiness and port publication: the `/proc/net` LISTEN probe, host-port
//! resolution, the published-ports scan, and `wait_ready` — the contract that `docker.run`
//! only returns a container that is actually answering.

use std::sync::Arc;
use crate::progress::{self, Kind, Progress};
use super::*;

/// Is anything LISTENing on `port`, on an address reachable from OUTSIDE the container?
///
/// `/proc/net/tcp{,6}` is the container's own kernel accounting — it reports what the process
/// inside actually bound, which is the only honest answer to "is it ready". A server bound to
/// LOOPBACK inside a container answers only itself, so it is NOT ready for a sibling or for the
/// host; init phases that briefly bind localhost before the real start are exactly the case a
/// naive check waves through.
///
/// Addresses are native-endian hex, so IPv4 127.0.0.1 (0x7F000001) renders as `0100007F` — the
/// trailing octet pair is the address's FIRST octet. State `0A` is TCP_LISTEN.
pub(super) fn listening_on(proc_net: &str, port: u16) -> bool {
    proc_net.lines().any(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 || f[3] != "0A" {
            return false;
        }
        let Some((addr, p)) = f[1].rsplit_once(':') else {
            return false;
        };
        if u16::from_str_radix(p, 16).ok() != Some(port) {
            return false;
        }
        !is_loopback_hex(addr)
    })
}

pub(super) fn is_loopback_hex(addr: &str) -> bool {
    match addr.len() {
        8 => addr.ends_with("7F"),                        // 127.0.0.0/8
        32 => addr == "00000000000000000000000001000000", // ::1
        _ => false,
    }
}

/// The mapped host port for `port` — the authoritative answer, for a caller that actually needs
/// one. Cache hit is the overwhelmingly common path; a miss re-asks the daemon, because under
/// load a mapping can arrive after `docker.run` returned.
///
/// A port that was never requested fails immediately: waiting could not help, and this is a real
/// case worth answering fast (a network-only resource legitimately publishes nothing, and
/// `docker_readiness.lua` asserts exactly that via `pcall`).
pub(super) async fn resolved_host_port(container: &Container, port: u16) -> mlua::Result<u16> {
    if let Some(hp) = container.ports.get(&port) {
        return Ok(*hp);
    }
    if !container.requested.contains(&port) {
        return Err(mlua::Error::RuntimeError(format!(
            "container port {port} was not published (docker.run was not asked to publish it)"
        )));
    }
    let late = published_ports(
        &container.client,
        &container.id,
        &[port],
        Duration::from_secs(15),
    )
    .await;
    if let Some(hp) = late.found.get(&port) {
        return Ok(*hp);
    }
    // Say which of the three things went wrong, not just "not published": the daemon would not
    // answer, the container died, or the mapping genuinely never appeared.
    let why = match (
        late.last_error,
        exited_status(&container.client, &container.id).await,
    ) {
        (Some(err), _) => format!(" — docker did not answer: {err}"),
        (None, Some(status)) => format!(" — container {status}"),
        (None, None) => format!(
            " — container is running but the mapping never appeared (docker reported ports: {})",
            late.last_seen.as_deref().unwrap_or("<none>")
        ),
    };
    Err(mlua::Error::RuntimeError(format!(
        "container port {port} was not published{why}"
    )))
}

/// What the daemon's port map says about one requested port.
///
/// The three cases must stay distinct, and telling them apart is the whole difficulty: they all
/// read as "no host port" to a caller, but they call for opposite responses. `NotYet` means keep
/// waiting; `BoundNothing` means waiting is futile and the container must be replaced.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PortState {
    /// A host port is bound. The normal answer.
    Published(u16),
    /// The daemon has answered about this port, and its answer is that nothing is bound to it —
    /// either an explicit null or an empty binding list. A stable wrong answer, not a pending
    /// one: this is the runtime defect that no amount of polling fixes.
    BoundNothing,
    /// The port is not in the map at all — the mapping is still being wired. Poll again.
    NotYet,
}

/// Classify one wanted port against a daemon port map. Pure, so the distinction above can be
/// proven against every shape the daemon produces without needing a daemon that misbehaves on
/// cue — the misbehaviour being roughly a one-in-750 event.
pub(super) fn classify_port(ports: &HashMap<String, Option<Vec<PortBinding>>>, want: u16) -> PortState {
    match ports.get(&format!("{want}/tcp")) {
        Some(Some(binds)) => match binds
            .first()
            .and_then(|b| b.host_port.as_ref())
            .and_then(|s| s.parse::<u16>().ok())
        {
            Some(hp) => PortState::Published(hp),
            // Present, but bound to nothing: an empty list, or an entry whose host port is
            // missing or unparseable. The daemon has spoken and the answer is "nothing".
            None => PortState::BoundNothing,
        },
        Some(None) => PortState::BoundNothing,
        None => PortState::NotYet,
    }
}

/// Read the host ports the daemon has assigned so far, polling until every wanted port has a
/// binding or `budget` runs out. Returns whatever it found — **never an error**.
///
/// Publishing is **not atomic with `start`**: the container is running before the daemon has
/// finished wiring its port mappings. Idle, that gap is imperceptible (measured: mappings are
/// present on the first inspect); under load it stretches, and how far depends on the runtime.
/// A single un-retried inspect therefore wins on one machine and loses on another — the
/// "works on mine" failure this polls away.
///
/// Returning partial results rather than failing is deliberate, and is the lesson from getting
/// this wrong once: a missing mapping only matters to a caller that actually wants a host port.
/// A network-only resource — reachable by alias, nothing published — is a legitimate topology
/// member, and making publication an eager precondition failed those containers for a fact they
/// never needed. Resolution is therefore best-effort here and authoritative in `host_port`.
pub(super) async fn published_ports(
    client: &Docker,
    id: &str,
    wanted: &[u16],
    budget: Duration,
) -> PortScan {
    const EVERY: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + budget;
    let mut scan = PortScan::default();
    if wanted.is_empty() {
        return scan;
    }
    let mut exited = false;
    loop {
        match client.inspect_container(id, None).await {
            Ok(info) => {
                scan.last_error = None;
                // Liveness comes from the SAME response as the port map. It used to be a second
                // `inspect` per iteration, which doubled this loop's load on the daemon for a
                // fact the first call already carried — and this loop is not gentle: every 50ms,
                // per container, across every worker.
                exited = matches!(info.state.as_ref().and_then(|s| s.running), Some(false));
                if let Some(ports) = info.network_settings.and_then(|ns| ns.ports) {
                    // Keep what the daemon actually said. When this whole scan comes up empty
                    // the raw map is the evidence that separates "the key is absent" (the
                    // mapping has not been wired yet) from `"9000/tcp": null` (the daemon
                    // accepted the container but the binding failed) — two different bugs that
                    // look identical from a missing-port error alone.
                    scan.last_seen = Some(
                        ports
                            .iter()
                            .map(|(k, v)| match v {
                                Some(b) => format!("{k}={b:?}"),
                                None => format!("{k}=null"),
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    scan.bound_empty.clear();
                    for want in wanted {
                        if scan.found.contains_key(want) {
                            continue;
                        }
                        match classify_port(&ports, *want) {
                            PortState::Published(hp) => {
                                scan.found.insert(*want, hp);
                            }
                            PortState::BoundNothing => {
                                scan.bound_empty.insert(*want);
                            }
                            PortState::NotYet => {}
                        }
                    }
                }
            }
            // Keep retrying — a daemon under load can refuse or time out a single inspect — but
            // REMEMBER why. Silently swallowing this is how "the port was never published" and
            // "we could not ask" become the same, undiagnosable message.
            Err(e) => scan.last_error = Some(e.to_string()),
        }
        // Stop early on success, on a dead container (it will never publish anything more), or
        // when the budget is spent. The caller decides whether a partial answer is a problem.
        if scan.found.len() == wanted.len() || Instant::now() >= deadline || exited {
            return scan;
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// What a port scan learned: the mappings found, and the last inspect error if the daemon was
/// not answering — which is a different failure from a port that is genuinely unpublished.
#[derive(Default)]
pub(super) struct PortScan {
    pub(super) found: HashMap<u16, u16>,
    /// Ports the daemon reported with an EMPTY binding list — exposed, but bound to nothing.
    /// A stable wrong answer rather than a pending one, so the caller recreates instead of
    /// waiting. Recomputed each inspect, so it only ever describes the latest answer.
    pub(super) bound_empty: std::collections::HashSet<u16>,
    pub(super) last_error: Option<String>,
    /// The raw port map from the last successful inspect — evidence for the rare case where a
    /// running container never gets a mapping.
    pub(super) last_seen: Option<String>,
}

/// The outcome of asking a container's own kernel whether a port is listening.
///
/// The three cases must stay distinct. Collapsing `Failed` into `Unsupported` (as a bare
/// `Option` does) means one slow or refused exec — routine while a container is still coming
/// up — permanently downgrades readiness to the coarse host-port check for the rest of the wait.
pub(super) enum Probe {
    /// The container answered: this is the truth about whether the port is listening.
    Answered(bool),
    /// The image has no `cat`/procfs (scratch, distroless). It can never answer; stop asking.
    Unsupported,
    /// The exec itself failed — container not accepting execs *yet*, or a transient daemon
    /// error. Says nothing about readiness, and nothing about future attempts. Ask again.
    Failed,
}

/// Ask the container's kernel whether `port` is listening.
pub(super) async fn listening_in_container(container: &Container, port: u16) -> Probe {
    let cmd = vec![
        "cat".to_string(),
        "/proc/net/tcp".to_string(),
        "/proc/net/tcp6".to_string(),
    ];
    // A missing /proc/net/tcp6 makes `cat` exit non-zero while still printing tcp — so judge by
    // the output, not the exit code.
    let Ok((_, out, _)) = container_exec(&container.client, &container.id, cmd, None).await
    else {
        return Probe::Failed;
    };
    if !out.contains("local_address") {
        return Probe::Unsupported; // not a procfs table: this image can never answer
    }
    Probe::Answered(listening_on(&out, port))
}

/// Is the container still running? `Some(status)` describes a container that has *stopped*, for
/// use in an error; `None` means it is still running (or the daemon could not tell us, which we
/// treat as "keep waiting" rather than inventing a failure).
pub(super) async fn exited_status(client: &Docker, id: &str) -> Option<String> {
    let state = client.inspect_container(id, None).await.ok()?.state?;
    // Treat "the daemon did not tell us whether it is running" as running — the same answer as
    // before, but reached deliberately. Writing this as `state.running?` silently produced it
    // via `?` on a `None`, so a response we failed to understand was indistinguishable from a
    // healthy container, and would have been reported as "running but the mapping never
    // appeared" — a confident, wrong diagnosis.
    match state.running {
        Some(false) => {}
        Some(true) | None => return None,
    }
    let code = state.exit_code.unwrap_or_default();
    Some(match state.error.filter(|e| !e.is_empty()) {
        Some(err) => format!("exited with code {code} ({err})"),
        None => format!("exited with code {code}"),
    })
}

/// Whether the port's **host mapping** accepts a connection — the half of readiness the
/// in-container probe cannot see.
///
/// An UNPUBLISHED port is ready by definition: an in-network-only resource is reached by DNS
/// from a peer container and has no host mapping to wait on. Returning false there would hang
/// every such resource until timeout.
pub(super) async fn host_mapping_ready(container: &Container, port: u16) -> bool {
    match container.ports.get(&port) {
        Some(&host_port) => tokio::net::TcpStream::connect(("127.0.0.1", host_port))
            .await
            .is_ok(),
        None => true,
    }
}

pub(super) async fn wait_ready(
    container: &Container,
    wait: &Wait,
    progress: &Arc<dyn Progress>,
    image: &str,
) -> mlua::Result<()> {
    // Pause #4: a readiness poll is silent for up to a minute, and it is the pause most likely
    // to be mistaken for a wedge — the container is already up, so nothing else is obviously
    // happening. Held by scope: `wait_ready` returns from three places (ready, timeout, probe
    // failure) and `Activity`'s Drop closes all three, so no exit can strand an open line.
    let _activity = progress::start(progress, Kind::Waiting, image.to_string());
    let deadline = Instant::now() + wait.timeout;
    // Whether the in-container probe is supported — latched OFF only on a definitive
    // `Unsupported`, never on a transient `Failed`.
    //
    // An image with no `cat`/procfs (scratch, distroless — `traefik/whoami` is one) can never
    // answer, and re-asking every 250ms fires hundreds of failing exec round-trips across a long
    // wait. That is not just waste: under parallel docker load it is slow enough to consume the
    // readiness budget itself, turning a cheap fallback into a timeout. So: ask once, and if the
    // image *cannot* answer, use the coarse host-port check for the rest of the wait. An exec
    // that merely failed is a different thing and must not latch anything.
    let mut probe_supported = true;
    loop {
        let ready = if let Some(cmd) = &wait.cmd {
            // Run the author's readiness command in the container: ready ⇔ exit 0. This is the
            // honest signal for a server whose listening socket predates its ability to serve
            // (Postgres: `pg_isready` returns non-zero while the postmaster is still starting up,
            // so it does not race the way a `port` probe does). An exec that fails to *launch* —
            // container not accepting execs yet, or the command absent (127) — is "not ready yet",
            // not a hard error: retry until the deadline, then the timeout error tails the logs.
            match container_exec(&container.client, &container.id, cmd.clone(), None).await {
                Ok((0, _, _)) => true,
                Ok(_) | Err(_) => false,
            }
        } else if let Some(port) = wait.port {
            // Ask the CONTAINER, not the host. Connecting to the mapped host port is worthless as
            // a readiness signal: Docker Desktop's port proxy binds and accepts the moment the
            // container starts, so the check passes while the server is still booting — and never
            // fails at all for a container that never listens. It also cannot see an UNPUBLISHED
            // port, which an in-network-only resource legitimately has.
            let asked = if probe_supported {
                listening_in_container(container, port).await
            } else {
                Probe::Unsupported
            };
            match asked {
                // Listening inside is necessary but NOT sufficient. The test dials the
                // *published* port, and a host-side forwarder can lag the container as easily
                // as it can lead it: Docker Desktop's proxy accepts before the server is up
                // (which is why the container is asked at all), while OrbStack's refuses for a
                // beat after it is. Each check alone is a false positive on one runtime — the
                // container answers "is the server up", the mapping answers "can the test
                // reach it", and readiness is both.
                Probe::Answered(listening) => {
                    listening && host_mapping_ready(container, port).await
                }
                // Retry next tick; this says nothing about readiness either way.
                Probe::Failed => false,
                // The image cannot answer (no `cat`/procfs). Fall back to the coarse host-port
                // check — no worse than before, but do not pretend it is a true signal.
                Probe::Unsupported => {
                    probe_supported = false;
                    match container.ports.get(&port) {
                        Some(&host_port) => {
                            tokio::net::TcpStream::connect(("127.0.0.1", host_port))
                                .await
                                .is_ok()
                        }
                        None => false,
                    }
                }
            }
        } else if let Some(pattern) = &wait.log {
            container_logs(&container.client, &container.id)
                .await?
                .contains(pattern.as_str())
        } else {
            true
        };
        if ready {
            return Ok(());
        }
        // A container that has EXITED will never become ready. Waiting out the full timeout to
        // say "not ready" hides the actual failure (a bad command, a missing env var, a crash)
        // behind a slow, uninformative error. Check liveness only after the readiness probe came
        // back false, so a container that became ready and exited immediately still counts.
        if let Some(status) = exited_status(&container.client, &container.id).await {
            return Err(mlua::Error::RuntimeError(format!(
                "docker.run: container {} {status} before becoming ready{}",
                container.id,
                tail_logs(&container.client, &container.id).await
            )));
        }
        if Instant::now() >= deadline {
            return Err(mlua::Error::RuntimeError(format!(
                "docker.run: container {} not ready within {:?}{}",
                container.id,
                wait.timeout,
                tail_logs(&container.client, &container.id).await
            )));
        }
        tokio::time::sleep(wait.every).await;
    }
}
