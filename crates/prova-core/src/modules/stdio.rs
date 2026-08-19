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

use super::shell::CommandSpec;
use super::turn::{Codec, Framing, Selector};
use super::wiretap::TranscriptRow;
use crate::model::parse_duration;

/// The same default bound every other driver read carries.
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("spawn", spawn_fn(lua)?)?;
    Ok(t)
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
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

#[cfg(test)]
mod tests;
