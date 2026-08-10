//! The MIT reference placement broker — `prova broker`.
//!
//! **This is spec scaffolding, not part of using prova.** Prova alone answers `requires` and
//! `resources` in-process — single-machine, zero configuration, no socket, and that stays the
//! default forever. Installing a clustered broker (Anemnez Fleet's fleetd, or any implementation
//! that passes the conformance suite) and naming its socket is what makes the same suites
//! pool-aware. Most users never run this verb and never need to know it exists; the conformance
//! suite spawns it per proof and throws it away.
//!
//! docs/design/placement.md is the spec; `proofs/spec/placement/` is the contract this binary is
//! held to, exactly as any third-party or commercial broker is. Why it exists at all:
//!
//! - **It keeps the spec attestable.** The placement proofs must pass in prova's own CI forever,
//!   and they cannot depend on a proprietary broker. This is what lets `prova attest` answer for
//!   placement.md on any unix machine.
//! - **It is the second implementation.** A protocol with one implementation is a description of
//!   that implementation's quirks. The conformance suite stays honest only while something other
//!   than fleetd passes it — and a broker implementer reads this file instead of reverse
//!   engineering a product.
//! - **Local `materialize` is worktree isolation** — run a suite against an isolated tree at a jj
//!   change id while you keep editing. A single-machine capability, not distribution machinery.
//!
//! And what it deliberately is NOT: a pool of one, by construction. No discovery, no pairing, no
//! trust, no cross-node anything — every capability here is a strict subset of what prova already
//! does in-process. Multi-machine is the clustered broker's whole job.
//!
//! Deliberately synchronous: one thread per connection, `std::process` for `exec`. The protocol is
//! turn-based per connection (one request, one terminal frame), so async buys nothing here, and a
//! clustered broker is expected to replace this wholesale rather than grow out of it.
//!
//! What the local node offers is declared on the command line (`--offer <kind>`), mirroring how a
//! fleet node declares its slots in config. There is no default offer: a slot nobody declared is
//! `unsatisfiable`, and making that explicit here keeps the reference broker from silently granting
//! kinds a real pool would refuse.

#![cfg(unix)]

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// The protocol this broker speaks. A client's major must match; its minor must be `<=` ours.
const PROTOCOL_MAJOR: u64 = 1;
const PROTOCOL_MINOR: u64 = 0;

/// How long a granted lease lives when the claim named no `ttl_ms`.
const DEFAULT_TTL_MS: u64 = 300_000;

/// What a `busy` response tells the client to wait before retrying.
const RETRY_AFTER_MS: u64 = 1_000;

pub fn run(args: Vec<String>) -> ExitCode {
    let mut socket: Option<PathBuf> = None;
    let mut offers: Vec<String> = Vec::new();

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => match it.next() {
                Some(p) => socket = Some(PathBuf::from(p)),
                None => return usage("--socket needs a path"),
            },
            "--offer" => match it.next() {
                Some(kind) => offers.push(kind),
                None => return usage("--offer needs a slot kind"),
            },
            "--help" | "-h" => {
                println!(
                    "prova broker — the single-machine reference placement broker (docs/design/placement.md)\n\n\
                     usage: prova broker --socket <path> [--offer <kind>]...\n\n\
                     Spec scaffolding: the conformance suite (proofs/spec/placement/) spawns this so\n\
                     the placement protocol stays proven on any unix machine, and broker implementers\n\
                     read it as the working example. It is a pool of ONE by construction — using prova\n\
                     never requires it, and multi-machine placement is a clustered broker's job\n\
                     (install one and name its socket; the same suites become pool-aware).\n\n\
                     Serves the placement protocol (newline-delimited JSON) on a unix socket.\n\
                     `--offer` declares a slot kind this node owns; repeat it per kind. A kind\n\
                     never offered is `unsatisfiable` to claim — exactly as in a real pool."
                );
                return ExitCode::SUCCESS;
            }
            other => return usage(&format!("unknown flag {other:?}")),
        }
    }

    let Some(socket) = socket else {
        return usage("--socket is required (a unix socket path, kept short: SUN_LEN caps it near 104 bytes)");
    };
    if socket.exists() {
        // Never steal an address: a stale file and a live broker look identical from here, and
        // binding over a live one would strand its clients mid-lease.
        eprintln!(
            "prova broker: {} already exists — remove it if stale, or choose another path",
            socket.display()
        );
        return ExitCode::FAILURE;
    }

    let listener = match UnixListener::bind(&socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("prova broker: cannot bind {}: {e}", socket.display());
            return ExitCode::FAILURE;
        }
    };

    let state = Arc::new(Broker::new(offers));
    println!("prova broker: listening on unix://{}", socket.display());
    // The parent that spawned us may be scraping for readiness.
    let _ = std::io::stdout().flush();

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || serve(stream, state));
            }
            Err(e) => {
                eprintln!("prova broker: accept failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("prova broker: {problem} (see `prova broker --help`)");
    ExitCode::FAILURE
}

// ── state ─────────────────────────────────────────────────────────────────────────────────────

struct Broker {
    /// Slot kinds this node owns. The value is unused today (capacity is one writer or any number
    /// of readers, the spec's slot model); a map so a capacity knob has somewhere to land.
    offers: BTreeMap<String, ()>,
    leases: Mutex<HashMap<String, Lease>>,
    /// Materialized trees, keyed by change id — what makes re-materializing the same change
    /// idempotent. The creating lease bounds each workspace's lifetime.
    workspaces: Mutex<HashMap<String, Workspace>>,
    /// Version probes shell out; ask each tool once per broker lifetime.
    versions: Mutex<HashMap<String, semver::Version>>,
    next_lease: AtomicU64,
}

struct Lease {
    kind: String,
    exclusive: bool,
    ttl_ms: u64,
    expires_at_ms: u64,
}

struct Workspace {
    path: PathBuf,
    name: String,
    source: PathBuf,
    creator_lease: String,
}

impl Broker {
    fn new(offers: Vec<String>) -> Self {
        Broker {
            offers: offers.into_iter().map(|k| (k, ())).collect(),
            leases: Mutex::new(HashMap::new()),
            workspaces: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
            next_lease: AtomicU64::new(1),
        }
    }

    /// Drop expired leases and the workspaces their deaths orphan. Called lazily at the top of
    /// every lease-touching op — a killed client's slot comes back the first time anyone asks.
    fn reap(&self) {
        let now = now_ms();
        let dead: Vec<String> = {
            let leases = lock(&self.leases);
            leases
                .iter()
                .filter(|(_, l)| l.expires_at_ms <= now)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in dead {
            self.drop_lease(&id);
        }
    }

    /// Remove one lease and clean up everything whose lifetime it bounded.
    fn drop_lease(&self, id: &str) {
        lock(&self.leases).remove(id);
        let orphaned: Vec<(String, Workspace)> = {
            let mut ws = lock(&self.workspaces);
            let keys: Vec<String> = ws
                .iter()
                .filter(|(_, w)| w.creator_lease == id)
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .filter_map(|k| ws.remove(&k).map(|w| (k, w)))
                .collect()
        };
        for (_, w) in orphaned {
            // Forget before delete: jj refuses to forget a workspace whose directory it cannot
            // reconcile in some states, but a forgotten workspace's directory is always deletable.
            let _ = std::process::Command::new("jj")
                .args(["workspace", "forget", &w.name])
                .current_dir(&w.source)
                .output();
            let _ = std::fs::remove_dir_all(&w.path);
        }
    }
}

/// Take a broker lock, recovering from poisoning: every guarded value is a plain map (leases,
/// workspaces, probed versions), valid at every step — a panicked handler already reported its
/// own failure, and the broker must keep serving the pool.
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── the connection loop ───────────────────────────────────────────────────────────────────────

fn serve(stream: UnixStream, broker: Arc<Broker>) {
    let reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(_) => return,
    };
    let mut writer = stream;
    let mut said_hello = false;

    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }

        let frame: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                // A parse error must not kill the connection: leases are held across turns, and
                // dropping them as a side effect of a typo would release slots the client still
                // believes it holds.
                let _ = send(
                    &mut writer,
                    json!({ "id": null, "ok": false, "outcome": "error",
                            "message": format!("malformed frame: {e}") }),
                );
                continue;
            }
        };
        let id = frame.get("id").cloned().unwrap_or(Value::Null);
        let op = frame.get("op").and_then(Value::as_str).unwrap_or("");

        let response = if !said_hello && op != "hello" {
            error(&id, format!("say hello first (got {op:?} before hello)"))
        } else {
            match op {
                "hello" => {
                    let r = hello(&id, &frame);
                    said_hello = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    r
                }
                "resolve" => resolve(&broker, &id, &frame),
                "claim" => claim(&broker, &id, &frame),
                "renew" => renew(&broker, &id, &frame),
                "release" => release(&broker, &id, &frame),
                "exec" => exec(&broker, &id, &frame, &mut writer),
                "materialize" => materialize(&broker, &id, &frame),
                other => error(&id, format!("unknown op {other:?}")),
            }
        };
        if send(&mut writer, response).is_err() {
            return;
        }
    }
}

fn send(writer: &mut UnixStream, frame: Value) -> std::io::Result<()> {
    writeln!(writer, "{frame}")
}

fn error(id: &Value, message: String) -> Value {
    json!({ "id": id, "ok": false, "outcome": "error", "message": message })
}

// ── hello ─────────────────────────────────────────────────────────────────────────────────────

fn hello(id: &Value, frame: &Value) -> Value {
    let asked = frame.get("protocol").and_then(Value::as_str).unwrap_or("");
    let parsed: Option<(u64, u64)> = asked
        .split_once('.')
        .and_then(|(maj, min)| Some((maj.parse().ok()?, min.parse().ok()?)));
    match parsed {
        // `<=` is the spec's rule ("a broker MUST accept any client whose minor is <= its own") —
        // it only looks degenerate while PROTOCOL_MINOR is 0, and `==` would silently break the
        // first client one minor behind us the day it is not.
        #[allow(clippy::absurd_extreme_comparisons)]
        Some((major, minor)) if major == PROTOCOL_MAJOR && minor <= PROTOCOL_MINOR => {
            json!({
                "id": id, "ok": true,
                "protocol": format!("{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}"),
                "broker": format!("prova/{}", env!("CARGO_PKG_VERSION")),
                "features": ["exec", "materialize"],
                "nodes": 1,
            })
        }
        Some(_) => error(
            id,
            format!(
                "cannot speak protocol {asked:?} (this broker speaks {PROTOCOL_MAJOR}.{PROTOCOL_MINOR})"
            ),
        ),
        None => error(id, format!("malformed protocol version {asked:?}")),
    }
}

// ── resolve ───────────────────────────────────────────────────────────────────────────────────

/// One capability from a `resolve` or `claim`: `{ "name": …, "constraint": … }`.
struct Capability {
    name: String,
    constraint: Option<String>,
}

fn capabilities_of(frame: &Value) -> Result<Vec<Capability>, String> {
    let Some(list) = frame.get("capabilities") else {
        return Ok(Vec::new());
    };
    let Some(list) = list.as_array() else {
        return Err("capabilities must be a list".into());
    };
    list.iter()
        .map(|c| {
            let name = c
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "a capability needs a name".to_string())?;
            Ok(Capability {
                name: name.to_string(),
                constraint: c
                    .get("constraint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

fn resolve(broker: &Broker, id: &Value, frame: &Value) -> Value {
    let caps = match capabilities_of(frame) {
        Ok(caps) => caps,
        Err(e) => return error(id, e),
    };
    match unsatisfied(broker, &caps) {
        Ok(None) => json!({ "id": id, "ok": true, "outcome": "granted", "nodes": 1 }),
        Ok(Some(reason)) => {
            json!({ "id": id, "ok": false, "outcome": "unsatisfiable", "reason": reason })
        }
        Err(e) => error(id, e),
    }
}

/// Why this node cannot serve `caps`, or `None` when it can. Conjunctive by construction: the
/// first unmet capability names the whole answer, because "one node has all of them" is the
/// question `requires` asks.
fn unsatisfied(broker: &Broker, caps: &[Capability]) -> Result<Option<String>, String> {
    for cap in caps {
        if which(&cap.name).is_none() {
            return Ok(Some(format!("{:?} is unavailable", cap.name)));
        }
        if let Some(constraint) = &cap.constraint {
            let req = semver::VersionReq::parse(constraint)
                .map_err(|e| format!("malformed constraint {constraint:?} for {:?}: {e}", cap.name))?;
            let version = probed_version(broker, &cap.name);
            if !req.matches(&version) {
                return Ok(Some(format!(
                    "{} {version} does not satisfy {req}",
                    cap.name
                )));
            }
        }
    }
    Ok(None)
}

/// Is `name` an executable on this node's PATH?
///
/// Deliberately broader than the engine's local capability set (which is registered-or-builtin):
/// a pool node advertises its *tools*, and the conformance vocabulary (`sh`) assumes exactly this.
fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The version `name` reports, probed once per broker lifetime.
///
/// A tool that answers `--version` gets its first `major[.minor[.patch]]` token; one that does not
/// compares as `0.0.0` — a conservative floor that never overclaims (no real constraint like
/// `>= 9` is met by it) while still granting the vacuous `>= 0` that mere existence satisfies.
/// "Cannot confirm" refusing everything would make every constrained capability unsatisfiable on
/// nodes whose `sh` is dash, which the conformance suite correctly refuses to credit.
fn probed_version(broker: &Broker, name: &str) -> semver::Version {
    if let Some(v) = lock(&broker.versions).get(name) {
        return v.clone();
    }
    let output = std::process::Command::new(name)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output();
    let text = output
        .map(|o| {
            let mut t = String::from_utf8_lossy(&o.stdout).into_owned();
            t.push_str(&String::from_utf8_lossy(&o.stderr));
            t
        })
        .unwrap_or_default();
    let version = first_version_token(&text).unwrap_or_else(|| semver::Version::new(0, 0, 0));
    lock(&broker.versions)
        .insert(name.to_string(), version.clone());
    version
}

/// The first thing in `text` that reads as a version: a word starting with a digit, of which the
/// leading `digits(.digits)*` run is taken and padded to `major.minor.patch`.
fn first_version_token(text: &str) -> Option<semver::Version> {
    for word in text.split(|c: char| c.is_whitespace() || c == ',' || c == '(') {
        if !word.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let run: String = word
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let mut parts = run.trim_end_matches('.').splitn(3, '.');
        let major: u64 = parts.next()?.parse().ok()?;
        let minor: u64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let patch: u64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        return Some(semver::Version::new(major, minor, patch));
    }
    None
}

// ── claim / renew / release ───────────────────────────────────────────────────────────────────

fn claim(broker: &Broker, id: &Value, frame: &Value) -> Value {
    broker.reap();

    let Some(kind) = frame.get("kind").and_then(Value::as_str) else {
        return error(id, "claim needs a kind".into());
    };
    let exclusive = match frame.get("mode").and_then(Value::as_str).unwrap_or("exclusive") {
        "exclusive" => true,
        "shared" => false,
        other => return error(id, format!("unknown mode {other:?}")),
    };

    // Capabilities gate the claim exactly as they gate resolve — a slot on a node that cannot run
    // the work is not a grant, it is a trap.
    let caps = match capabilities_of(frame) {
        Ok(caps) => caps,
        Err(e) => return error(id, e),
    };
    match unsatisfied(broker, &caps) {
        Ok(None) => {}
        Ok(Some(reason)) => {
            return json!({ "id": id, "ok": false, "outcome": "unsatisfiable", "reason": reason })
        }
        Err(e) => return error(id, e),
    }

    if !broker.offers.contains_key(kind) {
        // The mirror of the busy rule: a slot nobody offers will never come free, so telling the
        // client to retry would hang the run forever. Absence skips; contention waits.
        return json!({ "id": id, "ok": false, "outcome": "unsatisfiable",
                       "reason": format!("no node offers slot kind {kind:?}") });
    }

    let mut leases = lock(&broker.leases);
    let contended = leases.values().any(|l| {
        l.kind == kind && (l.exclusive || exclusive) // writer blocks all; anyone blocks a writer
    });
    if contended {
        return json!({ "id": id, "ok": false, "outcome": "busy",
                       "retry_after_ms": RETRY_AFTER_MS });
    }

    let ttl_ms = frame.get("ttl_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TTL_MS);
    let lease_id = format!("L-{:x}", broker.next_lease.fetch_add(1, Ordering::Relaxed));
    let expires_at_ms = now_ms() + ttl_ms;
    leases.insert(
        lease_id.clone(),
        Lease { kind: kind.to_string(), exclusive, ttl_ms, expires_at_ms },
    );
    json!({ "id": id, "ok": true, "outcome": "granted",
            "lease": lease_id, "node": "local", "expires_at_ms": expires_at_ms })
}

fn renew(broker: &Broker, id: &Value, frame: &Value) -> Value {
    broker.reap();
    let Some(lease_id) = frame.get("lease").and_then(Value::as_str) else {
        return error(id, "renew needs a lease".into());
    };
    let mut leases = lock(&broker.leases);
    match leases.get_mut(lease_id) {
        Some(lease) => {
            // Extend by the lease's own TTL: the holder asked for this cadence at claim time.
            lease.expires_at_ms = now_ms() + lease.ttl_ms;
            json!({ "id": id, "ok": true, "expires_at_ms": lease.expires_at_ms })
        }
        // Expired or never existed — the slot may already be held by someone else, and a silent
        // re-grant is how a lease system double-books.
        None => error(id, format!("unknown or expired lease {lease_id:?}")),
    }
}

fn release(broker: &Broker, id: &Value, frame: &Value) -> Value {
    let Some(lease_id) = frame.get("lease").and_then(Value::as_str) else {
        return error(id, "release needs a lease".into());
    };
    // Idempotent by contract: teardown paths run twice more often than anyone intends, and the
    // second release of a slot you no longer hold is correct cleanup, not an error.
    broker.drop_lease(lease_id);
    json!({ "id": id, "ok": true })
}

/// The lease named by `frame`, if it exists and has not expired — the gate `exec` and
/// `materialize` share.
fn live_lease(broker: &Broker, frame: &Value) -> Result<String, String> {
    broker.reap();
    let lease_id = frame
        .get("lease")
        .and_then(Value::as_str)
        .ok_or_else(|| "this op needs a lease".to_string())?;
    let leases = lock(&broker.leases);
    if leases.contains_key(lease_id) {
        Ok(lease_id.to_string())
    } else {
        Err(format!("unknown or expired lease {lease_id:?}"))
    }
}

// ── exec ──────────────────────────────────────────────────────────────────────────────────────

fn exec(broker: &Broker, id: &Value, frame: &Value, writer: &mut UnixStream) -> Value {
    if let Err(e) = live_lease(broker, frame) {
        return error(id, e);
    }
    let argv: Vec<String> = frame
        .get("argv")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let Some((program, args)) = argv.split_first() else {
        return error(id, "exec needs a non-empty argv".into());
    };

    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(cwd) = frame.get("cwd").and_then(Value::as_str) {
        cmd.current_dir(cwd);
    }
    if let Some(env) = frame.get("env").and_then(Value::as_object) {
        for (k, v) in env {
            if let Some(v) = v.as_str() {
                cmd.env(k, v);
            }
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        // The TRANSPORT failed — distinct from the work failing, which is `ok: true` with the
        // work's own exit code below. Collapsing them makes every red test look like a broken
        // pool, and the fixes are not the same.
        Err(e) => return error(id, format!("cannot exec {program:?}: {e}")),
    };

    // Stream both pipes as they produce, interleaved on the one connection. Buffering to
    // completion would make a long remote suite look hung, which is why events exist at all.
    let (tx, rx) = std::sync::mpsc::channel::<(&'static str, String)>();
    let mut pumps = Vec::new();
    if let Some(out) = child.stdout.take() {
        pumps.push(pump("stdout", out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        pumps.push(pump("stderr", err, tx.clone()));
    }
    drop(tx);
    for (stream, data) in rx {
        let _ = send(writer, json!({ "id": id, "event": stream, "data": data }));
    }
    for pump in pumps {
        let _ = pump.join();
    }

    match child.wait() {
        Ok(status) => json!({ "id": id, "ok": true, "exit": status.code().unwrap_or(-1) }),
        Err(e) => error(id, format!("waiting on {program:?}: {e}")),
    }
}

/// Forward one pipe to the event channel, chunk by chunk, until it closes.
fn pump(
    stream: &'static str,
    mut source: impl Read + Send + 'static,
    tx: std::sync::mpsc::Sender<(&'static str, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match source.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if tx.send((stream, data)).is_err() {
                        return;
                    }
                }
            }
        }
    })
}

// ── materialize ───────────────────────────────────────────────────────────────────────────────

fn materialize(broker: &Broker, id: &Value, frame: &Value) -> Value {
    let lease_id = match live_lease(broker, frame) {
        Ok(l) => l,
        Err(e) => return error(id, e),
    };
    let vcs = frame.get("vcs").and_then(Value::as_str).unwrap_or("");
    if vcs != "jj" {
        return error(id, format!("unsupported vcs {vcs:?} (this broker speaks jj)"));
    }
    let Some(change) = frame.get("change").and_then(Value::as_str) else {
        return error(id, "materialize needs a change id".into());
    };
    let Some(source) = frame.get("source").and_then(Value::as_str) else {
        return error(id, "materialize needs a source".into());
    };

    // Idempotent per change: asking twice must not rebuild the world — that is what makes a retry
    // cheap, and what the warmth report is built on.
    if let Some(ws) = lock(&broker.workspaces).get(change) {
        return json!({ "id": id, "ok": true, "path": ws.path,
                       "warmth": { "shared_ancestor": change } });
    }

    let short: String = change.chars().take(12).collect();
    let name = format!("prova-broker-{}-{short}", std::process::id());
    let dir = std::env::temp_dir().join(format!("prova-broker-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return error(id, format!("cannot create workspace root: {e}"));
    }
    let path = dir.join(format!("ws-{short}"));

    // `jj workspace add -r <change>` parents the new workspace's working copy on the change, so
    // the tree at `path` IS the change's tree. Place by change id, never by branch name: an id is
    // content-addressed and means exactly one tree everywhere.
    let output = std::process::Command::new("jj")
        .args(["workspace", "add", "--name", &name, "-r", change])
        .arg(&path)
        .current_dir(source)
        .output();
    match output {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            // Whatever cannot be fetched must be refused. Silently materializing something else —
            // trunk, an empty tree — would run a suite that proves nothing about the code meant.
            //
            // And clean up what the refusal leaves: `jj workspace add` is not atomic — it can
            // register the workspace before `-r` fails to resolve, stranding a stub in the shared
            // store. (Observed on the suite's unknown-change proof, first run.)
            let _ = std::process::Command::new("jj")
                .args(["workspace", "forget", &name])
                .current_dir(source)
                .output();
            let _ = std::fs::remove_dir_all(&path);
            let stderr = String::from_utf8_lossy(&o.stderr);
            return error(id, format!("cannot materialize {change:?}: {}", stderr.trim()));
        }
        Err(e) => return error(id, format!("cannot run jj: {e}")),
    }

    lock(&broker.workspaces).insert(
        change.to_string(),
        Workspace {
            path: path.clone(),
            name,
            source: PathBuf::from(source),
            creator_lease: lease_id,
        },
    );
    // Local warmth is maximal and honestly so: the workspace shares the source's store, so the
    // nearest common ancestor of "requested" and "already here" is the change itself.
    json!({ "id": id, "ok": true, "path": path,
            "warmth": { "shared_ancestor": change } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tokens_parse_from_real_tool_banners() {
        let bash = first_version_token("GNU bash, version 3.2.57(1)-release (arm64-apple)").unwrap();
        assert_eq!(bash, semver::Version::new(3, 2, 57));

        let two_part = first_version_token("jq-1.7").unwrap_or(semver::Version::new(0, 0, 0));
        // "jq-1.7" starts with a letter — the parser takes the first word STARTING with a digit.
        assert_eq!(two_part, semver::Version::new(0, 0, 0));

        let plain = first_version_token("9.0.301").unwrap();
        assert_eq!(plain, semver::Version::new(9, 0, 301));

        assert!(first_version_token("no digits here").is_none());
    }

    #[test]
    fn unversioned_tools_compare_as_the_conservative_floor() {
        // The semantics the conformance suite discriminates: a floor of 0 is met by existence,
        // a real floor is never met by a tool that cannot report a version.
        let floor = semver::VersionReq::parse(">= 0").unwrap();
        let real = semver::VersionReq::parse(">= 9999").unwrap();
        let unknown = semver::Version::new(0, 0, 0);
        assert!(floor.matches(&unknown));
        assert!(!real.matches(&unknown));
    }
}
