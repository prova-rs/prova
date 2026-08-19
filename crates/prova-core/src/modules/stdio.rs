//! The `stdio` kernel transport — a CONVERSATION with a spawned process over its pipes
//! (docs/plans/stdio-transport.md, docs/design/mocks-proxies-drivers.md).
//!
//! `shell.spawn` boots a thing and probes it from outside; its stdin is nulled on purpose, and
//! that is the right shape for a server you drive over HTTP. It is the wrong shape for a SUT whose
//! protocol IS a conversation over stdio — an MCP server, an LSP server, a REPL, a debug adapter —
//! where the next thing you write depends on what came back
//! (`agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut`).
//!
//! Batching the requests instead is not a workaround, it is a race: a server free to dispatch
//! concurrently will answer turn two before turn one has stored anything, and the proof goes red
//! for a reason that has nothing to do with the behavior under test.
//!
//! **`stdio` and `terminal` are siblings, differing only in pty allocation** — which is exactly
//! why they are two namespaces and not one with a `pty = false` dial. `terminal`'s observation
//! layer IS the screen; a pty-less terminal would carry `:screen()`, `:cell()` and `:resize()` as
//! nil-returning holes, and an option that changes the type is worse than two types. A pty would
//! also corrupt the thing being carried here: line discipline, echo, and 80-column wrapping mangle
//! long JSON-RPC lines.
//!
//! The turn model — framing, codec, the `where` selector — is [`super::turn`], shared with
//! `socket` and `websocket`, so a proof learns one grammar for every stream.
//!
//! **The third stream is the design's one genuinely new problem.** A socket has two directions; a
//! process has three, and stderr must never enter the frame stream — a server logging to stdout's
//! sibling is normal, and folding it in would feed log lines to a JSON decoder as protocol
//! garbage. So stderr is a separate bounded tail, and it is in every read failure's message,
//! because "it logged a stack trace and stopped answering" is the dominant way these SUTs fail.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, ChildStdout};

use super::err;
use super::shell::CommandSpec;
use super::turn::{Codec, Framing, Selector};
use super::wiretap::TranscriptRow;
use crate::model::parse_duration;

/// The same default bound every other driver read carries.
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("spawn", spawn_fn(lua)?)?;
    t.set("mock", mock_fn(lua)?)?;
    t.set("proxy", proxy_fn(lua)?)?;
    Ok(t)
}

// ── the driver: Session (originate) ────────────────────────────────────────────────────────────

/// One live conversation with a spawned process.
///
/// Each half is `take()`n out for the duration of an I/O call, exactly as `socket`'s `Conn` does,
/// so a concurrent call errors ("busy") instead of panicking a `RefCell` across an await.
struct Session {
    child: Rc<RefCell<Option<tokio::process::Child>>>,
    stdin: Rc<RefCell<Option<ChildStdin>>>,
    stdout: Rc<RefCell<Option<ChildStdout>>>,
    /// Bytes read past the last complete frame — the next `recv` consumes them first.
    buf: Rc<RefCell<Vec<u8>>>,
    /// The diagnostic tail, filled by a reader task. `Arc<Mutex>` rather than `Rc<RefCell>`
    /// because the reader is a spawned task, exactly as `shell.spawn`'s output capture is.
    stderr: Arc<Mutex<Vec<u8>>>,
    transcript: Rc<RefCell<Vec<TranscriptRow>>>,
    framing: Framing,
    codec: Codec,
    pid: Option<u32>,
    /// Held while the process runs, so prova's own death sweeps a still-live session
    /// (docs/design/verifiers.md#conduct-lease-survives-prova-death).
    lease: RefCell<Option<crate::lease::Lease>>,
    /// What the caller asked to run, for error messages — a failure that does not say WHICH
    /// process went silent is a failure you have to bisect.
    label: String,
}

impl Session {
    fn stderr_tail(&self) -> String {
        let b = self.stderr.lock().unwrap_or_else(|p| p.into_inner());
        String::from_utf8_lossy(&b).trim_end().to_string()
    }

    /// The child's state right now, named precisely enough to separate the three failures a silent
    /// process can be in: it exited, it is alive and not answering, or we cannot tell.
    fn child_status(&self) -> String {
        match self.child.borrow_mut().as_mut() {
            None => "already reaped".to_string(),
            Some(c) => match c.try_wait() {
                Ok(Some(status)) => format!("exited ({status})"),
                Ok(None) => match self.pid {
                    Some(p) => format!("still running (pid {p})"),
                    None => "still running".to_string(),
                },
                Err(e) => format!("status unknown ({e})"),
            },
        }
    }

    /// The message every bounded read fails with.
    ///
    /// An empty read is the least informative thing this API can report, and on its own it cannot
    /// distinguish "the program never started" from "it started and said nothing" from "it wrote
    /// to the other stream and died". Reporting those three facts — turns seen, child status, and
    /// the stderr tail — is what makes the first recurrence explain itself instead of needing a
    /// bisect. (`terminal:expect` learned this the same way.)
    fn diagnose(&self, what: &str, turns: usize, extra: &str) -> mlua::Error {
        let stderr = self.stderr_tail();
        let stderr = if stderr.is_empty() {
            "-- stderr: (silent) --".to_string()
        } else {
            format!("-- stderr (tail) --\n{stderr}")
        };
        err(format!(
            "{what}{extra}\n-- stdio: {} turns read, child {} [{}] --\n{stderr}",
            turns,
            self.child_status(),
            self.label,
        ))
    }
}

/// Check the read half out of the session for the duration of one I/O call.
///
/// The stream is taken rather than borrowed so a concurrent call errors ("busy") instead of
/// panicking a `RefCell` across an await — `socket`'s `Conn` does the same. Both readers below
/// pair this with [`restore`], and the pairing is why a timed-out read leaves a session that still
/// works rather than one permanently busy.
fn checkout(this: &Session, what: &str) -> mlua::Result<(ChildStdout, Vec<u8>)> {
    let Some(out) = this.stdout.borrow_mut().take() else {
        return Err(err(format!("{what}: session is closed or busy")));
    };
    Ok((out, std::mem::take(&mut *this.buf.borrow_mut())))
}

fn restore(this: &Session, out: ChildStdout, buf: Vec<u8>) {
    *this.buf.borrow_mut() = buf;
    *this.stdout.borrow_mut() = Some(out);
}

/// Scan `buf` for `needle`, pulling more bytes until it appears. The unframed half of `expect`.
async fn scan_raw<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    needle: &[u8],
) -> std::io::Result<bool> {
    loop {
        if buf.windows(needle.len()).any(|w| w == needle) {
            return Ok(true);
        }
        let mut chunk = [0u8; 16 * 1024];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(false);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Every option `recv` and `expect` honor.
const READ_OPTS: &[&str] = &["timeout", "where"];
const EXPECT_OPTS: &[&str] = &["timeout"];

fn read_args(this: &Session, opts: &Option<Table>) -> mlua::Result<(Duration, Selector)> {
    let Some(t) = opts else {
        return Ok((DEFAULT_IO_TIMEOUT, Selector::Any));
    };
    crate::opts::reject_unknown(t, READ_OPTS, "recv")?;
    let dur = match t.get::<Option<String>>("timeout")? {
        Some(s) => parse_duration(&s).ok_or_else(|| err(format!("recv: bad duration {s:?}")))?,
        None => DEFAULT_IO_TIMEOUT,
    };
    let sel = Selector::parse("recv", this.codec, t.get::<Option<Value>>("where")?)?;
    if this.framing.is_raw() && !sel.is_any() {
        return Err(err(
            "recv: `where` selects among TURNS, and this session is unframed — set framing so the \
             stream has turns to choose from",
        ));
    }
    Ok((dur, sel))
}

fn opt_timeout(opts: &Option<Table>, site: &str, default: Duration) -> mlua::Result<Duration> {
    let Some(t) = opts else { return Ok(default) };
    crate::opts::reject_unknown(t, EXPECT_OPTS, site)?;
    match t.get::<Option<String>>("timeout")? {
        Some(s) => parse_duration(&s).ok_or_else(|| err(format!("{site}: bad duration {s:?}"))),
        None => Ok(default),
    }
}

/// The three verb families the Session contract names (docs/plans/stdio-transport.md §3), split
/// the way `terminal` splits its own: **drive** (`:send`), **observe** (`:recv`, `:expect`,
/// `:stderr`, `:transcript`), **lifecycle** (`:eof`, `:wait`, `:stop`). The split is the contract
/// made visible in the code — a reader looking for "how does this transport observe?" lands in one
/// function rather than scanning one long one.
impl UserData for Session {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("pid", |_, this| Ok(this.pid));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_drive_methods(methods);
        add_observe_methods(methods);
        add_lifecycle_methods(methods);
    }
}

/// Drive: write one turn, framed and encoded.
fn add_drive_methods<M: UserDataMethods<Session>>(methods: &mut M) {
    methods.add_async_method("send", |lua, this, data: Value| async move {
        let payload = this.codec.encode(&lua, &data)?;
        let wire = this.framing.encode(&payload);
        let Some(mut w) = this.stdin.borrow_mut().take() else {
            // Distinguishable from "busy" on purpose: after `:eof()` this is the author's own
            // doing, and saying so is faster than saying the stream is unavailable.
            return Err(err(
                "send: stdin is closed (`:eof()` was called, or the session was stopped)",
            ));
        };
        this.transcript.borrow_mut().push(TranscriptRow {
            dir: "in",
            data: payload,
        });
        let r = w.write_all(&wire).await;
        // A pipe is buffered; without the flush the peer may never see the turn we are about
        // to block waiting for a reply to.
        let r = match r {
            Ok(()) => w.flush().await,
            e => e,
        };
        *this.stdin.borrow_mut() = Some(w);
        r.map_err(|e| err(format!("send: {e}")))
    });

}

/// Observe: the two bounded reads, plus the two evidence surfaces.
fn add_observe_methods<M: UserDataMethods<Session>>(methods: &mut M) {
    methods.add_async_method("recv", |lua, this, opts: Option<Table>| async move {
        let (dur, sel) = read_args(&this, &opts)?;
        if this.framing.is_raw() {
            return Err(err(
                "recv: this session is unframed, so there are no turns to read — set framing \
                 (\"line\" for newline-delimited JSON, \"content_length\" for LSP), or use \
                 :expect(pattern) to scan the raw stream",
            ));
        }
        let mut skipped = 0usize;
        let (mut out, mut buf) = checkout(&this, "recv")?;
        let (framing, codec) = (this.framing.clone(), this.codec);
        let transcript = this.transcript.clone();
        let res = tokio::time::timeout(
            dur,
            super::turn::read_until(&mut out, &mut buf, &framing, |payload| {
                // Every turn read is transcript, matched or not: the notification that arrived
                // while we waited for the reply is evidence, not noise.
                transcript.borrow_mut().push(TranscriptRow {
                    dir: "out",
                    data: payload.to_vec(),
                });
                if sel.is_any() {
                    return Ok(true);
                }
                let hit = sel.accepts(&codec.decode(&lua, payload)?)?;
                if !hit {
                    skipped += 1;
                }
                Ok(hit)
            }),
        )
        .await;
        restore(&this, out, buf);
        let turns = this.transcript.borrow().len();
        match res {
            Err(_) => Err(this.diagnose(
                "recv: timed out",
                turns,
                &format!(" after {dur:?}{}", super::turn::waited(skipped)),
            )),
            Ok(Err(e)) => Err(e),
            Ok(Ok(None)) => Err(this.diagnose(
                "recv: the stream ended without producing a turn",
                turns,
                &super::turn::waited(skipped),
            )),
            Ok(Ok(Some(payload))) => this.codec.decode(&lua, &payload),
        }
    });

    // Observe-until-match, the unframed sibling of `recv{ where = … }`: block until the
    // stream SHOWS something. On a framed session it scans turns; on a raw one, bytes.
    methods.add_async_method(
        "expect",
        |_, this, (pattern, opts): (mlua::String, Option<Table>)| async move {
            let dur = opt_timeout(&opts, "expect", DEFAULT_IO_TIMEOUT)?;
            let needle = pattern.as_bytes().to_vec();
            let quoted = String::from_utf8_lossy(&needle).to_string();
            let (mut out, mut buf) = checkout(&this, "expect")?;
            let framing = this.framing.clone();
            let transcript = this.transcript.clone();
            let res = tokio::time::timeout(dur, async {
                if framing.is_raw() {
                    return scan_raw(&mut out, &mut buf, &needle)
                        .await
                        .map_err(|e| err(format!("expect: {e}")));
                }
                super::turn::read_until(&mut out, &mut buf, &framing, |payload| {
                    transcript.borrow_mut().push(TranscriptRow {
                        dir: "out",
                        data: payload.to_vec(),
                    });
                    Ok(needle.is_empty() || payload.windows(needle.len()).any(|w| w == needle))
                })
                .await
                .map(|hit| hit.is_some())
            })
            .await;
            restore(&this, out, buf);
            let turns = this.transcript.borrow().len();
            match res {
                Err(_) => Err(this.diagnose(
                    &format!("expect {quoted:?}: not observed"),
                    turns,
                    &format!(" within {dur:?}"),
                )),
                Ok(Err(e)) => Err(e),
                Ok(Ok(false)) => Err(this.diagnose(
                    &format!("expect {quoted:?}: the stream ended without producing it"),
                    turns,
                    "",
                )),
                Ok(Ok(true)) => Ok(()),
            }
        },
    );

    // The bounded diagnostic tail (last 64KB, oldest dropped). Never part of the frame
    // stream — asserting on a server's LOGS is a different act from reading its protocol.
    methods.add_method("stderr", |lua, this, ()| {
        let b = this.stderr.lock().unwrap_or_else(|p| p.into_inner());
        lua.create_string(&b[..])
    });

    super::wiretap::add_transcript_method(methods);
}

/// Lifecycle: half-close, reap, kill.
fn add_lifecycle_methods<M: UserDataMethods<Session>>(methods: &mut M) {
    // Half-close stdin. A distinct act from `:stop()` and worth its own verb: "the client went
    // away" is a real obligation for these SUTs, and `sess:eof(); sess:wait()` is how a proof
    // states that the server shuts down cleanly when it happens.
    methods.add_method("eof", |_, this, ()| {
        this.stdin.borrow_mut().take();
        Ok(())
    });

    methods.add_async_method("wait", |_, this, opts: Option<Table>| async move {
        let dur = opt_timeout(&opts, "wait", Duration::from_secs(30))?;
        let Some(mut child) = this.child.borrow_mut().take() else {
            return Ok(None); // already reaped
        };
        let res = tokio::time::timeout(dur, child.wait()).await;
        match res {
            Err(_) => {
                // Put it back: a timed-out wait has not reaped anything, and a session whose
                // child vanished from under it could never be stopped.
                *this.child.borrow_mut() = Some(child);
                Err(this.diagnose(
                    "wait: the process is still running",
                    this.transcript.borrow().len(),
                    &format!(" after {dur:?} (did you forget `:eof()`?)"),
                ))
            }
            Ok(Err(e)) => Err(err(format!("wait: {e}"))),
            Ok(Ok(status)) => {
                this.lease.borrow_mut().take(); // exited — nothing left to sweep
                Ok(status.code())
            }
        }
    });

    // What `ctx:manage` calls at scope end. Idempotent.
    methods.add_async_method("stop", |_, this, ()| async move {
        this.stdin.borrow_mut().take();
        let child = this.child.borrow_mut().take();
        if let Some(mut child) = child {
            // The session is its own process GROUP, so a server's workers die with it —
            // exactly as `shell.spawn` and every bounded conduct do
            // (docs/design/verifiers.md#conduct-process-group-reaping).
            crate::lease::kill_group(this.pid);
            let _ = child.kill().await;
        }
        this.lease.borrow_mut().take();
        Ok(())
    });
}

// Not the `impl_transcript!` macro: that one reaches through a `state` field, which is a mock/proxy
// shape. A driver session holds its transcript directly — one direction-tagged row per turn, in or
// out — and the `transcript()` METHOD is still the shared one, so the evidence shape cannot drift.
impl super::wiretap::ProxyTranscript for Session {
    fn transcript_rows(&self) -> Vec<TranscriptRow> {
        self.transcript.borrow().clone()
    }
}

/// Every option `stdio.spawn` honors — closed by construction. `cmd` carries the arguments (there
/// is no `args`, for the reason `shell.spawn` has none: a dropped `args` still STARTS the process,
/// leaving a proof that drives a different program than it reads).
const SPAWN_OPTS: &[&str] = &["cmd", "codec", "cwd", "env", "framing"];

fn spawn_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        super::runtime_only("stdio.spawn")?;
        let opts = opts.ok_or_else(|| {
            err("stdio.spawn(ctx, { cmd = … }): the options table is required")
        })?;
        crate::opts::reject_unknown(&opts, SPAWN_OPTS, "stdio.spawn")?;

        let cmd = CommandSpec::parse(opts.get::<Value>("cmd")?)
            .map_err(|e| err(format!("stdio.spawn(ctx, {{ cmd = … }}): {e}")))?;
        let framing = Framing::parse("stdio.spawn", opts.get::<Option<Value>>("framing")?)?;
        let codec = Codec::parse("stdio.spawn", opts.get::<Option<Value>>("codec")?)?;
        let label = cmd.display_name();

        let mut command = cmd.build();
        if let Some(dir) = opts.get::<Option<String>>("cwd")? {
            command.current_dir(dir);
        }
        if let Some(env) = opts.get::<Option<Table>>("env")? {
            for pair in env.pairs::<String, Value>() {
                let (k, v) = pair?;
                let v = super::shell::env_value(&k, v)?;
                command.env(k, v);
            }
        }
        command
            // The whole point: a pipe we own, not the harness's stdin.
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // SEPARATE, never merged into stdout. A server logging to stderr is normal; folding
            // the two would feed log lines to the frame reader as protocol garbage.
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        super::shell::isolate_group(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| err(format!("stdio.spawn {label}: {e}")))?;
        let pid = child.id();
        let lease = crate::lease::Lease::register(pid);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        if let Some(e) = child.stderr.take() {
            super::shell::spawn_output_reader(e, stderr_buf.clone());
        }

        let ud = lua.create_userdata(Session {
            child: Rc::new(RefCell::new(Some(child))),
            stdin: Rc::new(RefCell::new(stdin)),
            stdout: Rc::new(RefCell::new(stdout)),
            buf: Rc::new(RefCell::new(Vec::new())),
            stderr: stderr_buf,
            transcript: Rc::new(RefCell::new(Vec::new())),
            framing,
            codec,
            pid,
            lease: RefCell::new(Some(lease)),
            label,
        })?;
        super::manage("stdio.spawn", &ctx, &ud)?;
        Ok(ud)
    })
}


// ── the spawnable adapter: mock (terminate) and proxy (interpose) ──────────────────────────────
//
// **A stdio mock IS a socket mock, reached by spawn instead of dial.** The transport definition in
// `mocks-proxies-drivers.md` already says a transport can "listen, connect-OR-SPAWN"; the listen
// postures simply never exercised the spawn half. A SUT that spawns its dependency cannot dial an
// address, so something must exist on PATH — and that something is an ADAPTER, with no business
// knowing what a stub is.
//
// So the shim is two lines and carries no behavior:
//
//     #!/bin/sh
//     exec "/abs/path/to/prova" relay --to "unix:///…/mock.sock"
//
// Everything that decides anything — stubs, journal, faults, cassettes — stays in-process where
// the real `Selector` lives. That inversion is what makes shape matching possible at all: the
// older shims (`terminal.mock`, `shell.proxy`) render behavior INTO `sh`, which is exactly why
// their matching is stuck at `case` patterns over bytes.
//
// Three properties worth naming, because they follow from the shape rather than being extras:
//
//   * **The spawn race is closed by construction.** `Acceptor::bind` binds synchronously and is
//     already accepting when it returns, so the socket is live before the shim path — and
//     therefore before anything could spawn it — exists at all.
//   * **prova is referenced by ABSOLUTE path**, so the SUT's PATH gains only the shim's directory.
//     Shadowing a command name is the point; putting prova itself on the SUT's PATH would be a
//     hermeticity change nobody asked for.
//   * **The relay never needs updating.** It is a byte pump; framing and codec happen at both
//     ends, so a new framing costs it nothing.

/// Write the two-line launcher that makes a listening endpoint spawnable under `name`.
fn write_relay_shim(
    dir: &std::path::Path,
    name: &str,
    addr: &str,
) -> mlua::Result<std::path::PathBuf> {
    let shim = dir.join(name);
    let exe = std::env::current_exe()
        .map_err(|e| err(format!("stdio: locating this prova to point the shim at: {e}")))?;
    let script = format!(
        "#!/bin/sh\n\
         # generated by prova — the spawnable adapter (docs/plans/stdio-transport.md §4).\n\
         # No behavior lives here on purpose: this is a byte pump, and everything that decides\n\
         # anything is in the prova process listening on the socket below.\n\
         exec {} relay --to {}\n",
        sh_quote(exe.to_string_lossy().as_ref()),
        sh_quote(addr),
    );
    std::fs::write(&shim, script).map_err(|e| err(format!("stdio: writing shim: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| err(format!("stdio: chmod shim: {e}")))?;
    }
    Ok(shim)
}

/// Single-quote for `sh`, closing and reopening around embedded quotes.
fn sh_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// One shim dir per spawnable INSTANCE, first on PATH; the socket lives beside the shim.
///
/// Keyed on a per-process counter rather than on the shadowed name, because two spawnables can
/// legitimately shadow the same name — a record proxy and the replay proxy that succeeds it are
/// the ordinary case. Sharing a directory made them share a socket PATH, and `Acceptor`'s `Drop`
/// reaps that path: the first proxy's teardown, landing asynchronously after the second had
/// already bound, deleted the socket out from under it. The SUT then failed to connect to a mock
/// that was listening perfectly well.
fn shim_dir(what: &str, name: &str) -> mlua::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NTH: AtomicU64 = AtomicU64::new(0);
    let nth = NTH.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "prova-{what}-{}-{nth}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)
        .map_err(|e| err(format!("stdio: mkdir {}: {e}", dir.display())))?;
    Ok(dir)
}

/// The PATH-prefixed environment to hand whatever spawns the SUT.
fn shim_env(lua: &Lua, dir: &std::path::Path) -> mlua::Result<mlua::RegistryKey> {
    let env = lua.create_table()?;
    let current = std::env::var("PATH").unwrap_or_default();
    env.set("PATH", format!("{}:{current}", dir.display()))?;
    lua.create_registry_value(env)
}

/// `as` — the command NAME to shadow. A path would be a different request (replace this file),
/// and shadowing works by putting a directory first on PATH, so only a bare name can mean it.
fn shadow_name(opts: &Table, site: &str) -> mlua::Result<String> {
    let name = opts
        .get::<Option<String>>("as")?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(format!("{site}(ctx, {{ as = \"name\" }}): `as` is required")))?;
    if name.contains('/') {
        return Err(err(format!("{site}: `as` is a command NAME, not a path")));
    }
    Ok(name)
}

// ── mock: terminate ────────────────────────────────────────────────────────────────────────────

/// A PATH-shadowed framed responder — `socket.mock` that the SUT reaches by spawning.
struct StdioMockUd {
    env: mlua::RegistryKey,
    shim: std::path::PathBuf,
    state: Rc<RefCell<super::socket::MockState>>,
    codec: Codec,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    sock_path: std::path::PathBuf,
}

impl super::wiretap::ShimHandle for StdioMockUd {
    fn env_key(&self) -> &mlua::RegistryKey {
        &self.env
    }
    fn shim_path(&self) -> String {
        self.shim.to_string_lossy().into_owned()
    }
}

impl super::wiretap::MockJournal for StdioMockUd {
    fn journal_rows(&self) -> Vec<super::wiretap::JournalRow> {
        super::socket::journal_of(&self.state)
    }
}

impl super::wiretap::Shutdown for StdioMockUd {
    fn take_shutdown(&self) -> Option<tokio::sync::oneshot::Sender<()>> {
        let taken = self.shutdown.borrow_mut().take();
        if taken.is_some() {
            // Reap the socket path so a re-mock under the same name can re-bind.
            let _ = std::fs::remove_file(&self.sock_path);
        }
        taken
    }
}

impl UserData for StdioMockUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        super::wiretap::add_shim_fields(fields);
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        super::socket::add_on_method(
            methods,
            |t: &StdioMockUd| &t.state,
            |t: &StdioMockUd| t.codec,
        );
        super::wiretap::add_received_method(methods);
        super::wiretap::add_shutdown_methods(methods);
    }
}

/// Every option `stdio.mock` honors.
const MOCK_OPTS: &[&str] = &["as", "codec", "framing"];

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        super::runtime_only("stdio.mock")?;
        let opts = opts.ok_or_else(|| {
            err("stdio.mock(ctx, { as = \"name\", framing = … }): the options table is required")
        })?;
        crate::opts::reject_unknown(&opts, MOCK_OPTS, "stdio.mock")?;
        let name = shadow_name(&opts, "stdio.mock")?;
        let framing = Framing::parse("stdio.mock", opts.get::<Option<Value>>("framing")?)?;
        if framing.is_raw() {
            return Err(err(
                "stdio.mock: framing is required — a mock matches TURNS, and raw bytes have no \
                 turn boundary",
            ));
        }
        let codec = Codec::parse("stdio.mock", opts.get::<Option<Value>>("codec")?)?;

        let dir = shim_dir("stdio-mock", &name)?;
        let sock_path = dir.join("mock.sock");
        let _ = std::fs::remove_file(&sock_path); // a dead run's socket must not block the bind
        let (acceptor, addr) = super::socket::bind_at(&format!("unix://{}", sock_path.display()))?;
        let state: Rc<RefCell<super::socket::MockState>> = Rc::default();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        super::socket::serve_mock(lua, acceptor, state.clone(), framing, codec, rx);

        // Written only AFTER the socket is accepting: the shim's existence is the signal that the
        // endpoint is reachable, so there is no window where a spawn could beat the listen.
        let shim = write_relay_shim(&dir, &name, &addr)?;
        let ud = lua.create_userdata(StdioMockUd {
            env: shim_env(lua, &dir)?,
            shim,
            state,
            codec,
            shutdown: RefCell::new(Some(tx)),
            sock_path,
        })?;
        super::manage("stdio.mock", &ctx, &ud)?;
        Ok(ud)
    })
}


// ── proxy: interpose ───────────────────────────────────────────────────────────────────────────
//
// The cell `shell.proxy` structurally cannot fill. Its turn is one whole INVOCATION (argv + stdin →
// stdout + exit), so an interleaved session collapses into a single opaque blob; a conversation
// needs turn-by-turn pairing. This is that pairing, over the shared cassette engine.
//
// One genuine difference from `socket.proxy`, and it is why this is not a straight delegation: a
// socket proxy's upstream is an ADDRESS it dials, while a stdio proxy's upstream is a COMMAND it
// SPAWNS — one real server per interposed session, so the session state each end keeps stays
// paired the way the SUT expects.

/// What the proxy does with each turn.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Forward to a real upstream and record nothing but the transcript.
    Passthrough,
    /// Forward, and capture each request→response pair into a cassette.
    Record,
    /// Answer from the cassette with no upstream at all. A miss is LOUD.
    Replay,
}

struct ProxyState {
    transcript: Vec<TranscriptRow>,
    recorder: Option<Rc<super::cassette::Recorder>>,
    /// Continuous per-turn delay. `corrupt`/`throttle` are deliberately absent — they are
    /// BYTE-level faults, and this proxy's unit is a turn; `socket.proxy` is where a byte-level
    /// wiretap lives. Same carve-out `http.proxy` and `grpc.proxy` make, for the same reason.
    latency: Option<Duration>,
    dropped: bool,
}

struct StdioProxyUd {
    env: mlua::RegistryKey,
    shim: std::path::PathBuf,
    state: Rc<RefCell<ProxyState>>,
    shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    sock_path: std::path::PathBuf,
}

impl super::wiretap::ShimHandle for StdioProxyUd {
    fn env_key(&self) -> &mlua::RegistryKey {
        &self.env
    }
    fn shim_path(&self) -> String {
        self.shim.to_string_lossy().into_owned()
    }
}

impl super::wiretap::ProxyTranscript for StdioProxyUd {
    fn transcript_rows(&self) -> Vec<TranscriptRow> {
        self.state.borrow().transcript.clone()
    }
}

impl UserData for StdioProxyUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        super::wiretap::add_shim_fields(fields);
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        super::wiretap::add_transcript_method(methods);

        methods.add_method("latency", |_, this, d: String| {
            let dur = parse_duration(&d)
                .ok_or_else(|| err(format!("latency: bad duration {d:?}")))?;
            this.state.borrow_mut().latency = Some(dur);
            Ok(())
        });
        methods.add_method("drop", |_, this, ()| {
            this.state.borrow_mut().dropped = true;
            Ok(())
        });
        // The byte-level half of the vocabulary, refused with where it lives rather than silently
        // accepted — a fault that reads as configured and injures nothing is worse than none.
        for verb in ["corrupt", "throttle"] {
            methods.add_method(verb, move |_, _, _: Value| -> mlua::Result<()> {
                Err(err(format!(
                    "stdio.proxy: `{verb}` is a BYTE-level fault and this proxy's unit is a turn — \
                     put a socket.proxy in front of a socket dependency for byte-level injury"
                )))
            });
        }

        // Teardown flushes a recording — the cassette's write point, exactly as socket.proxy's is.
        methods.add_method("stop", |_, this, ()| stop_proxy(this));
        methods.add_method("close", |_, this, ()| stop_proxy(this));
    }
}

fn stop_proxy(this: &StdioProxyUd) -> mlua::Result<()> {
    if let Some(tx) = this.shutdown.borrow_mut().take() {
        let _ = tx.send(());
        let _ = std::fs::remove_file(&this.sock_path);
        let recorder = this.state.borrow().recorder.clone();
        if let Some(r) = recorder {
            r.flush()
                .map_err(|e| err(format!("stdio.proxy: writing cassette: {e}")))?;
        }
    }
    Ok(())
}

/// Every option `stdio.proxy` honors.
const PROXY_OPTS: &[&str] = &["as", "cassette", "framing", "mode", "redact", "upstream"];

fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
        super::runtime_only("stdio.proxy")?;
        let opts = opts.ok_or_else(|| {
            err("stdio.proxy(ctx, { as = \"name\", upstream = … }): the options table is required")
        })?;
        crate::opts::reject_unknown(&opts, PROXY_OPTS, "stdio.proxy")?;
        let name = shadow_name(&opts, "stdio.proxy")?;
        let framing = Framing::parse("stdio.proxy", opts.get::<Option<Value>>("framing")?)?;
        if framing.is_raw() {
            return Err(err(
                "stdio.proxy: framing is required — an interposed SESSION is a sequence of turns, \
                 and raw bytes have no turn boundary to pair a request with its response",
            ));
        }
        let cassette = opts.get::<Option<String>>("cassette")?;
        let upstream = opts.get::<Option<Value>>("upstream")?;

        // `auto` collapses on the cassette's presence before anything downstream sees it, exactly
        // as socket.proxy and http.proxy do — so the pump only ever knows the three real modes.
        let mode = match opts.get::<Option<String>>("mode")?.as_deref().unwrap_or("passthrough") {
            "passthrough" => Mode::Passthrough,
            "record" => Mode::Record,
            "replay" => Mode::Replay,
            "auto" => match cassette.as_ref().is_some_and(|p| std::path::Path::new(p).exists()) {
                true => Mode::Replay,
                false => Mode::Record,
            },
            other => {
                return Err(err(format!(
                    "stdio.proxy: unknown mode {other:?} (passthrough|record|replay|auto)"
                )))
            }
        };
        if mode != Mode::Replay && upstream.is_none() {
            return Err(err(
                "stdio.proxy: this mode forwards, so it needs an `upstream` command to spawn \
                 (replay is the mode that needs none)",
            ));
        }
        // Bind the requirement to the VALUE rather than checking and then unwrapping later: the
        // "we already verified this" unwrap is how an invariant and its enforcement drift apart.
        let cassette = match (mode, cassette) {
            (Mode::Passthrough, c) => c,
            (_, Some(c)) => Some(c),
            (_, None) => {
                return Err(err("stdio.proxy: record/replay needs a `cassette` path"))
            }
        };
        let upstream = match upstream {
            Some(v) => Some(CommandSpec::parse(v)?),
            None => None,
        };

        let player = match (mode, &cassette) {
            (Mode::Replay, Some(path)) => Some(Rc::new(RefCell::new(
                super::cassette::Player::load_of(path, super::socket::BYTE_TURN_KINDS)
                    .map_err(|e| err(format!("stdio.proxy: {e}")))?,
            ))),
            _ => None,
        };
        let recorder = match (mode, &cassette) {
            (Mode::Record, Some(path)) => {
                let redact = opts.get::<Option<Vec<String>>>("redact")?.unwrap_or_default();
                Some(Rc::new(
                    super::cassette::Recorder::new(path.clone(), "stdio")
                        .with_redactions(redact),
                ))
            }
            _ => None,
        };

        let state = Rc::new(RefCell::new(ProxyState {
            transcript: Vec::new(),
            recorder,
            latency: None,
            dropped: false,
        }));

        let dir = shim_dir("stdio-proxy", &name)?;
        let sock_path = dir.join("proxy.sock");
        let _ = std::fs::remove_file(&sock_path);
        let (acceptor, addr) = super::socket::bind_at(&format!("unix://{}", sock_path.display()))?;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        serve_proxy(acceptor, state.clone(), framing, upstream, player, rx);

        let shim = write_relay_shim(&dir, &name, &addr)?;
        let ud = lua.create_userdata(StdioProxyUd {
            env: shim_env(lua, &dir)?,
            shim,
            state,
            shutdown: RefCell::new(Some(tx)),
            sock_path,
        })?;
        super::manage("stdio.proxy", &ctx, &ud)?;
        Ok(ud)
    })
}

/// Accept interposed sessions until shutdown; one upstream process per session.
fn serve_proxy(
    acceptor: super::socket::Acceptor,
    state: Rc<RefCell<ProxyState>>,
    framing: Framing,
    upstream: Option<CommandSpec>,
    player: Option<Rc<RefCell<super::cassette::Player>>>,
    mut rx: tokio::sync::oneshot::Receiver<()>,
) {
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = acceptor.accept() => {
                    let Ok(client) = accepted else { break };
                    let state = state.clone();
                    let framing = framing.clone();
                    let up = upstream.as_ref().map(|c| c.build());
                    let player = player.clone();
                    tokio::task::spawn_local(async move {
                        let _ = interpose(client, state, framing, up, player).await;
                    });
                }
            }
        }
    });
}

/// One interposed session: a synchronous request→response turn loop, which is what makes the
/// recording coherent (a bidirectional pump has no pairing, so there is nothing to key a cassette
/// on). Ends when the client hangs up, the upstream dies, or a replay misses.
async fn interpose(
    mut client: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    state: Rc<RefCell<ProxyState>>,
    framing: Framing,
    upstream: Option<tokio::process::Command>,
    player: Option<Rc<RefCell<super::cassette::Player>>>,
) -> std::io::Result<()> {
    let mut child = match upstream {
        Some(mut cmd) => {
            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true);
            Some(cmd.spawn()?)
        }
        None => None,
    };
    let mut up_in = child.as_mut().and_then(|c| c.stdin.take());
    let mut up_out = child.as_mut().and_then(|c| c.stdout.take());

    let (mut cbuf, mut ubuf) = (Vec::new(), Vec::new());
    while let Ok(Some(req)) = super::turn::read_frame(&mut client, &mut cbuf, &framing).await {
        {
            let mut s = state.borrow_mut();
            s.transcript.push(TranscriptRow { dir: "up", data: req.clone() });
            if s.dropped {
                return Ok(()); // the fuse blew: the SUT sees its server go away mid-session
            }
        }
        let latency = state.borrow().latency;
        if let Some(d) = latency {
            tokio::time::sleep(d).await;
        }

        let resp = match (&mut up_in, &mut up_out, &player) {
            // Forwarding: hand the turn to the real server and wait for its answer.
            (Some(w), Some(r), _) => {
                use tokio::io::AsyncWriteExt;
                w.write_all(&framing.encode(&req)).await?;
                w.flush().await?;
                match super::turn::read_frame(r, &mut ubuf, &framing).await? {
                    Some(resp) => resp,
                    None => return Ok(()), // upstream closed; the session is over
                }
            }
            // Replay: no upstream at all. A miss closes the connection LOUD rather than
            // inventing bytes — the client's next read errors, which is the point.
            (_, _, Some(p)) => {
                let key = super::cassette::encode_bytes(&req);
                match p.borrow_mut().answer(&key) {
                    Some(turn) => super::cassette::decode_bytes(&turn.response),
                    None => return Ok(()),
                }
            }
            _ => return Ok(()),
        };

        {
            let mut s = state.borrow_mut();
            s.transcript.push(TranscriptRow { dir: "down", data: resp.clone() });
            if let Some(rec) = s.recorder.clone() {
                rec.record(
                    super::cassette::encode_bytes(&req),
                    super::cassette::encode_bytes(&resp),
                    None,
                );
            }
        }
        use tokio::io::AsyncWriteExt;
        client.write_all(&framing.encode(&resp)).await?;
        client.flush().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
