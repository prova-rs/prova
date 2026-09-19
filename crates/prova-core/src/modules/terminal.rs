//! The `terminal` module — prova's Lua face over the terminal kernel (the `prova-terminal` crate,
//! docs/design/terminal-kernel.md): PTY-backed driving of interactive programs, with a screen model
//! as the observation layer (docs/design/mocks-proxies-drivers.md, proofs/spec/terminal).
//!
//! The kernel owns the SESSION — allocation (openpty on unix, ConPTY on Windows), the reader
//! thread, the transcript and the screen model — and answers every wait as a non-blocking check.
//! This face owns the POLICY: waits are bounded by a deadline (`timeout`), polled on the async
//! runtime, and every session is torn down via `ctx:manage`. `terminal` is the user-facing word.
//!
//! Driver surface: `terminal.spawn(ctx, { cmd, cols, rows, env? })` → a session with `:send`,
//! `:expect` (observe-until-match with a timeout — the same idea as `wait_for`; never a sleep),
//! `:wait_stable`, `:screen()` (→ `Screen`: text/line/cell/contains, snapshot-able), `:resize`
//! (a real SIGWINCH), `:signal`, `:wait`. Torn down via `ctx:manage` like every resource.
//!
//! Mock surface: `terminal.mock(ctx, { as = "name" })` shadows a CLI on PATH with a scripted
//! responder (expect→send pairs, generated as a self-contained POSIX shim). The narrow, true
//! mock: your SUT shells out to an interactive CLI and you script the other side. Mocks and
//! proxies are prova's alone — test-driver conveniences, not kernel concerns.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
use prova_terminal::{ExpectCheck, Session, SpawnSpec};

use crate::model::parse_duration;

const DEFAULT_WAIT: Duration = Duration::from_secs(10);
/// `:wait` only: a child's exit is not output, so the reap still polls `try_wait`. The output waits
/// (`:expect`, `:wait_stable`) wake on the kernel's per-chunk notification instead.
const POLL: Duration = Duration::from_millis(15);
/// `wait_stable`: the frame is settled when no new bytes arrive for this window.
const QUIET: Duration = Duration::from_millis(150);

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("spawn", spawn_fn(lua)?)?;
    t.set("mock", mock_fn(lua)?)?;
    t.set("proxy", proxy_fn(lua)?)?;
    Ok(t)
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

fn opt_timeout(opts: &Option<Table>, default: Duration) -> mlua::Result<Duration> {
    match opts {
        Some(t) => match t.get::<Option<String>>("timeout")? {
            Some(s) => parse_duration(&s).ok_or_else(|| err(format!("bad timeout {s:?}"))),
            None => Ok(default),
        },
        None => Ok(default),
    }
}

/// A live session. One `RefCell`, borrowed only between awaits — Lua runs one coroutine at a
/// time, so no two borrows ever overlap.
struct TermUd {
    session: RefCell<Session>,
    /// Woken by the kernel's reader after every output chunk and at end-of-stream
    /// (`Session::on_output`). `notify_one` keeps a permit when nobody is waiting, so output that
    /// lands between a check and the next await is never missed.
    wake: Arc<tokio::sync::Notify>,
}

/// A frozen frame — plain data copied out under the kernel's lock, so assertions never race the
/// reader.
struct ScreenUd(prova_terminal::Screen);

impl UserData for ScreenUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("rows", |_, this| Ok(this.0.rows));
        fields.add_field_method_get("cols", |_, this| Ok(this.0.cols));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("text", |_, this, ()| Ok(this.0.contents.clone()));
        // 0-based, like cell(row, col) — screen geometry is coordinates, not Lua arrays.
        methods.add_method("line", |_, this, n: usize| Ok(this.0.line(n).to_string()));
        methods.add_method("contains", |_, this, s: String| Ok(this.0.contains(&s)));
        methods.add_method("cell", |lua, this, (r, c): (usize, usize)| {
            let cell = this.0.cell(r, c).ok_or_else(|| {
                err(format!("cell({r}, {c}): outside the {}x{} screen", this.0.rows, this.0.cols))
            })?;
            let t = lua.create_table()?;
            t.set("char", cell.ch.clone())?;
            t.set("fg", cell.fg.clone())?;
            t.set("bg", cell.bg.clone())?;
            t.set("bold", cell.bold)?;
            Ok(t)
        });
        // The snapshot protocol: any userdata exposing `snapshot_text()` can be the subject of
        // `matches_snapshot` — a Screen snapshots as its rendered frame text.
        methods.add_method("snapshot_text", |_, this, ()| Ok(this.0.contents.clone()));
    }
}

/// Driving the session: `:send` raw bytes, `:expect` (observe-until-match with a timeout).
fn add_drive_methods<M: UserDataMethods<TermUd>>(methods: &mut M) {
    methods.add_method("send", |_, this, data: mlua::String| {
        this.session.borrow_mut().send(&data.as_bytes()).map_err(|e| err(format!("send: {e}")))
    });

    // Observe-until-match with a timeout — never a sleep. The kernel scans the raw transcript,
    // so a string that scrolled off the screen still counts as observed.
    methods.add_async_method(
        "expect",
        |_, this, (pattern, opts): (mlua::String, Option<Table>)| async move {
            let needle = pattern.as_bytes().to_vec();
            let dur = opt_timeout(&opts, DEFAULT_WAIT)?;
            let deadline = tokio::time::Instant::now() + dur;
            loop {
                match this.session.borrow().check_expect(&needle) {
                    ExpectCheck::Found => return Ok(()),
                    ExpectCheck::Ended { bytes, why, tail } => {
                        return Err(err(format!(
                            "expect {:?}: the stream ended without producing it \
                             [{bytes} bytes read, {why}] (transcript tail: {tail:?})",
                            String::from_utf8_lossy(&needle)
                        )));
                    }
                    ExpectCheck::Pending => {}
                }
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    // An empty screen is the least informative thing a pty failure can show,
                    // and on its own it cannot distinguish "the program never ran" from "it ran
                    // and said nothing" from "it spoke and we lost it". Report the facts that
                    // separate those, so the first recurrence explains itself instead of
                    // needing a bisect.
                    let stall = this.session.borrow_mut().diagnose();
                    return Err(err(format!(
                        "expect {:?}: not observed within {dur:?}\n\
                         -- pty: {} bytes read, reader {}, child {} --{}\n\
                         -- screen --\n{}",
                        String::from_utf8_lossy(&needle),
                        stall.bytes,
                        stall.reader,
                        stall.child,
                        stall.tree,
                        stall.screen
                    )));
                }
                // Sleep until the program writes or the deadline passes — never a fixed poll.
                tokio::select! {
                    () = this.wake.notified() => {}
                    () = tokio::time::sleep_until(deadline) => {}
                }
            }
        },
    );
}

/// Observing the frame: `:wait_stable`, `:screen()` snapshots, `:resize` (a real SIGWINCH).
fn add_observe_methods<M: UserDataMethods<TermUd>>(methods: &mut M) {
    // Settle the frame: done when no new output for a quiet window. The anti-sleep.
    methods.add_async_method("wait_stable", |_, this, opts: Option<Table>| async move {
        let dur = opt_timeout(&opts, DEFAULT_WAIT)?;
        let deadline = tokio::time::Instant::now() + dur;
        let mut last_len = this.session.borrow().activity().bytes;
        let mut quiet_since = tokio::time::Instant::now();
        loop {
            // Wake on new output, or when the quiet window (or the deadline) would close.
            let wake_at = (quiet_since + QUIET).min(deadline);
            tokio::select! {
                () = this.wake.notified() => {}
                () = tokio::time::sleep_until(wake_at) => {}
            }
            let now = this.session.borrow().activity();
            if now.bytes != last_len {
                last_len = now.bytes;
                quiet_since = tokio::time::Instant::now();
            } else if now.ended || quiet_since.elapsed() >= QUIET {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(err(format!("wait_stable: output never settled within {dur:?}")));
            }
        }
    });

    methods.add_method("screen", |lua, this, ()| {
        lua.create_userdata(ScreenUd(this.session.borrow().screen()))
    });

    // A real SIGWINCH: the pty is resized, the child is signaled, and the parser's geometry
    // follows — `stty size` inside the session reports the new numbers.
    methods.add_method("resize", |_, this, (cols, rows): (u16, u16)| {
        this.session.borrow_mut().resize(cols, rows).map_err(|e| err(format!("resize: {e}")))
    });
}

/// Session lifecycle: `:signal`, `:wait` (exit status + teardown backstop).
fn add_lifecycle_methods<M: UserDataMethods<TermUd>>(methods: &mut M) {
    methods.add_method("signal", |_, this, name: String| {
        this.session.borrow().signal(&name).map_err(|e| {
            err(match e {
                // kill(2)'s refusal names the signal it was given: `signal INT: kill(…) failed`.
                prova_terminal::Error::Kill { .. } => format!("signal {e}"),
                _ => format!("signal: {e}"),
            })
        })
    });

    // Reap the child and report its exit code. Polling try_wait keeps everything on the
    // single-threaded runtime — no blocking wait, no Send bound on the child handle.
    methods.add_async_method("wait", |lua, this, opts: Option<Table>| async move {
        let dur = opt_timeout(&opts, Duration::from_secs(30))?;
        let deadline = tokio::time::Instant::now() + dur;
        loop {
            let status =
                this.session.borrow_mut().try_wait().map_err(|e| err(format!("wait: {e}")))?;
            if let Some(s) = status {
                let t = lua.create_table()?;
                t.set("code", s.code)?;
                return Ok(t);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(err(format!("wait: child still running after {dur:?}")));
            }
            tokio::time::sleep(POLL).await;
        }
    });

    // `ctx:manage` teardown: kill the child, close the pty. Idempotent, LIFO, for free.
    methods.add_method("stop", |_, this, ()| {
        this.session.borrow_mut().stop();
        Ok(())
    });
}

impl UserData for TermUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_drive_methods(methods);
        add_observe_methods(methods);
        add_lifecycle_methods(methods);
    }
}

fn spawn_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        super::runtime_only("terminal.spawn")?;
        let cmd: Vec<String> = opts
            .get::<Option<Vec<String>>>("cmd")?
            .filter(|v| !v.is_empty())
            .ok_or_else(|| err("terminal.spawn(ctx, { cmd = { … } }): cmd is required"))?;
        let mut spec = SpawnSpec {
            cmd,
            cols: opts.get::<Option<u16>>("cols")?.unwrap_or(80),
            rows: opts.get::<Option<u16>>("rows")?.unwrap_or(24),
            cwd: opts.get::<Option<String>>("cwd")?,
            env: Vec::new(),
        };
        if let Some(env) = opts.get::<Option<Table>>("env")? {
            for pair in env.pairs::<String, String>() {
                spec.env.push(pair?);
            }
        }
        let session = Session::spawn(&spec).map_err(|e| {
            err(match e {
                // The program's own failure names it: `terminal.spawn "cat": …`.
                prova_terminal::Error::Spawn { .. } => format!("terminal.spawn {e}"),
                _ => format!("terminal.spawn: {e}"),
            })
        })?;
        let wake = Arc::new(tokio::sync::Notify::new());
        let hook = wake.clone();
        session.on_output(move || hook.notify_one());
        let ud = lua.create_userdata(TermUd { session: RefCell::new(session), wake })?;
        super::manage("terminal.spawn", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── terminal.mock: the PATH-shadow responder ───────────────────────────────────────────────────

struct MockCliState {
    dir: std::path::PathBuf,
    shim: std::path::PathBuf,
    /// expect→send pairs, checked in order; the first whose expect is contained in stdin answers.
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
}

struct MockCliUd {
    state: Rc<RefCell<MockCliState>>,
    env: mlua::RegistryKey,
}

struct MockCliStep {
    state: Rc<RefCell<MockCliState>>,
    expect: Vec<u8>,
}

fn sh_quote(bytes: &[u8]) -> String {
    // POSIX single-quote escaping: close, escaped quote, reopen.
    let s = String::from_utf8_lossy(bytes);
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// (Re)generate the shim as a self-contained POSIX responder. One-shot stdin conversation —
/// the honest v1 of the doc's scripted responder; a full interactive (pty-looped) responder
/// rides the same file when the need is proven.
fn write_shim(state: &MockCliState) -> mlua::Result<()> {
    let mut script = String::from(
        "#!/bin/sh\n# generated by prova terminal.mock — a scripted PATH-shadow responder\n\
         input=$(cat)\ncase \"$input\" in\n",
    );
    for (expect, send) in &state.pairs {
        script.push_str(&format!(
            "  *{}*) printf '%s' {} ; exit 0 ;;\n",
            sh_quote(expect),
            sh_quote(send)
        ));
    }
    script.push_str(
        "esac\necho 'prova terminal.mock: no scripted expectation matched stdin' >&2\nexit 1\n",
    );
    std::fs::write(&state.shim, script)
        .map_err(|e| err(format!("terminal.mock: writing shim: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state.shim, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| err(format!("terminal.mock: chmod shim: {e}")))?;
    }
    Ok(())
}

impl UserData for MockCliStep {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("send", |_, this, reply: mlua::String| {
            {
                let mut s = this.state.borrow_mut();
                s.pairs.push((this.expect.clone(), reply.as_bytes().to_vec()));
            }
            write_shim(&this.state.borrow())
        });
    }
}

impl UserData for MockCliUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("env", |lua, this| {
            lua.registry_value::<Table>(&this.env)
        });
        fields.add_field_method_get("path", |_, this| {
            Ok(this.state.borrow().shim.to_string_lossy().to_string())
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("expect", |lua, this, pattern: mlua::String| {
            lua.create_userdata(MockCliStep {
                state: this.state.clone(),
                expect: pattern.as_bytes().to_vec(),
            })
        });
        methods.add_method("stop", |_, this, ()| {
            let dir = this.state.borrow().dir.clone();
            let _ = std::fs::remove_dir_all(dir);
            Ok(())
        });
    }
}

/// Every option `terminal.mock` honors — closed by construction
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
const MOCK_OPTS: &[&str] = &["as"];

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        crate::opts::reject_unknown(&opts, MOCK_OPTS, "terminal.mock")?;
        let name = opts
            .get::<Option<String>>("as")?
            .ok_or_else(|| err("terminal.mock(ctx, { as = \"name\" }): `as` is required"))?;
        if name.contains('/') || name.contains('\\') {
            return Err(err("terminal.mock: `as` is a command NAME, not a path"));
        }
        // One shim dir per mock, first on PATH — teardown removes it.
        let dir = std::env::temp_dir().join(format!(
            "prova-term-mock-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| err(format!("terminal.mock: mkdir {}: {e}", dir.display())))?;
        let shim = dir.join(&name);
        let state = Rc::new(RefCell::new(MockCliState {
            dir: dir.clone(),
            shim,
            pairs: Vec::new(),
        }));
        write_shim(&state.borrow())?;

        // `env` merges over the spawner's environment (shell.run env semantics), so prepending
        // to the CURRENT PATH is both correct and hermetic-enough: the shim wins on name.
        let env = lua.create_table()?;
        let current = std::env::var("PATH").unwrap_or_default();
        env.set("PATH", format!("{}:{current}", dir.display()))?;
        let env_key = lua.create_registry_value(env)?;

        let ud = lua.create_userdata(MockCliUd {
            state,
            env: env_key,
        })?;
        super::manage("terminal.mock", &ctx, &ud)?;
        Ok(ud)
    })
}

// ── terminal.proxy: interpose on an interactive CLI (record/replay the session) ────────────────

/// The terminal cassette — the full-duplex, asciinema-shaped kind (docs/design/
/// mocks-proxies-drivers.md): the raw terminal output stream, VT sequences intact, so replay
/// reproduces a styled interactive session byte-for-byte. This is what makes the cross-platform
/// story work — a ConPTY session recorded once replays on every platform. v1 captures the output
/// frames; input-timed matching is the deeper form that rides the same file.
#[derive(serde::Serialize, serde::Deserialize)]
struct TermCassette {
    version: u32,
    kind: String,
    /// The recorded output, byte-lossless (base64 when not valid UTF-8).
    frames: String,
}

struct TermProxyState {
    dir: std::path::PathBuf,
    shim: std::path::PathBuf,
    /// Where the record shim spools the real program's output; wrapped into the cassette at close.
    raw: std::path::PathBuf,
    cassette: Option<String>,
    recording: bool,
}

struct TermProxyUd {
    state: Rc<RefCell<TermProxyState>>,
    env: mlua::RegistryKey,
}

fn write_term_shim(state: &TermProxyState, upstream: Option<&str>, replay_frames: Option<&std::path::Path>) -> mlua::Result<()> {
    let q = |s: &str| sh_quote(s.as_bytes());
    let script = if let Some(frames) = replay_frames {
        // Replay: reproduce the recorded output on the inherited pty. The real program never runs.
        format!("#!/bin/sh\ncat {}\n", q(&frames.to_string_lossy()))
    } else if let Some(up) = upstream {
        if state.recording {
            // Record: run the real program (inheriting the SUT's pty), spool its combined output,
            // then replay it to the pty so the invocation looks untouched. `:close()` wraps the
            // spool into the cassette.
            format!(
                "#!/bin/sh\n{cmd} \"$@\" > {raw} 2>&1\ncode=$?\ncat {raw}\nexit $code\n",
                cmd = q(up),
                raw = q(&state.raw.to_string_lossy())
            )
        } else {
            // Passthrough: forward, record nothing.
            format!("#!/bin/sh\nexec {} \"$@\"\n", q(up))
        }
    } else {
        return Err(err("terminal.proxy: no upstream and no replay frames (internal)"));
    };
    std::fs::write(&state.shim, script)
        .map_err(|e| err(format!("terminal.proxy: writing shim: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state.shim, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| err(format!("terminal.proxy: chmod shim: {e}")))?;
    }
    Ok(())
}

impl super::wiretap::ShimHandle for TermProxyUd {
    fn env_key(&self) -> &mlua::RegistryKey {
        &self.env
    }
    fn shim_path(&self) -> String {
        self.state.borrow().shim.to_string_lossy().to_string()
    }
}

impl UserData for TermProxyUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        super::wiretap::add_shim_fields(fields);
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("stop", |_, this, ()| term_proxy_stop(this));
        methods.add_method("close", |_, this, ()| term_proxy_stop(this));
    }
}

fn term_proxy_stop(this: &TermProxyUd) -> mlua::Result<()> {
    // Record mode: the flush point. Wrap the spooled raw output into the cassette (outside the
    // shim dir), then remove the shim dir.
    let (cassette, recording, raw, dir) = {
        let s = this.state.borrow();
        (s.cassette.clone(), s.recording, s.raw.clone(), s.dir.clone())
    };
    if recording {
        if let Some(path) = cassette {
            let bytes = std::fs::read(&raw).unwrap_or_default();
            let cas = TermCassette {
                version: 1,
                kind: "terminal".to_string(),
                frames: super::cassette::encode_bytes(&bytes),
            };
            let text = serde_json::to_string_pretty(&cas)
                .map_err(|e| err(format!("terminal.proxy: encoding cassette: {e}")))?;
            std::fs::write(&path, text)
                .map_err(|e| err(format!("terminal.proxy: writing cassette {path:?}: {e}")))?;
        }
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

/// Every option `terminal.proxy` honors — closed by construction
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
const PROXY_OPTS: &[&str] = &["as", "cassette", "mode", "upstream"];

fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        crate::opts::reject_unknown(&opts, PROXY_OPTS, "terminal.proxy")?;
        let name = opts
            .get::<Option<String>>("as")?
            .ok_or_else(|| err("terminal.proxy(ctx, { as = \"name\" }): `as` is required"))?;
        if name.contains('/') || name.contains('\\') {
            return Err(err("terminal.proxy: `as` is a command NAME, not a path"));
        }
        let upstream = opts.get::<Option<String>>("upstream")?;
        let cassette = opts.get::<Option<String>>("cassette")?;
        let mode_str = opts
            .get::<Option<String>>("mode")?
            .unwrap_or_else(|| "passthrough".to_string());

        let mode = match mode_str.as_str() {
            "passthrough" | "record" | "replay" => mode_str.as_str(),
            "auto" => {
                let cas = cassette
                    .as_ref()
                    .ok_or_else(|| err("terminal.proxy: mode \"auto\" needs a `cassette`"))?;
                if std::path::Path::new(cas).exists() {
                    "replay"
                } else {
                    "record"
                }
            }
            other => {
                return Err(err(format!(
                    "terminal.proxy: mode must be passthrough|record|replay|auto, got {other:?}"
                )))
            }
        };
        if mode != "passthrough" && cassette.is_none() {
            return Err(err(format!("terminal.proxy: mode {mode_str:?} needs a `cassette`")));
        }
        if mode == "record" && upstream.is_none() {
            return Err(err("terminal.proxy: recording needs an `upstream`"));
        }

        let dir = std::env::temp_dir().join(format!(
            "prova-term-proxy-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| err(format!("terminal.proxy: mkdir {}: {e}", dir.display())))?;
        let state = TermProxyState {
            shim: dir.join(&name),
            raw: dir.join("raw"),
            dir: dir.clone(),
            cassette: cassette.clone(),
            recording: mode == "record",
        };

        // Replay: decode the cassette's frames to a file the shim `cat`s; no upstream consulted.
        if mode == "replay" {
            let path = cassette
                .as_ref()
                .ok_or_else(|| err("terminal.proxy: mode \"replay\" needs a `cassette`"))?;
            let text = std::fs::read_to_string(path)
                .map_err(|e| err(format!("terminal.proxy: reading cassette: {e}")))?;
            let cas: TermCassette = serde_json::from_str(&text)
                .map_err(|e| err(format!("terminal.proxy: parsing cassette: {e}")))?;
            let frames_path = dir.join("frames");
            std::fs::write(&frames_path, super::cassette::decode_bytes(&cas.frames))
                .map_err(|e| err(format!("terminal.proxy: staging replay frames: {e}")))?;
            write_term_shim(&state, None, Some(&frames_path))?;
        } else {
            write_term_shim(&state, upstream.as_deref(), None)?;
        }

        let env = lua.create_table()?;
        let current = std::env::var("PATH").unwrap_or_default();
        env.set("PATH", format!("{}:{current}", dir.display()))?;
        let env_key = lua.create_registry_value(env)?;

        let ud = lua.create_userdata(TermProxyUd {
            state: Rc::new(RefCell::new(state)),
            env: env_key,
        })?;
        super::manage("terminal.proxy", &ctx, &ud)?;
        Ok(ud)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_timeout_parses_or_defaults() {
        let lua = Lua::new();
        let default = Duration::from_secs(5);
        assert_eq!(opt_timeout(&None, default).unwrap(), default);
        let t = lua.create_table().unwrap();
        assert_eq!(opt_timeout(&Some(t), default).unwrap(), default, "opts without timeout");
        let t = lua.create_table().unwrap();
        t.set("timeout", "150ms").unwrap();
        assert_eq!(opt_timeout(&Some(t), default).unwrap(), Duration::from_millis(150));
        let t = lua.create_table().unwrap();
        t.set("timeout", "soon").unwrap();
        assert!(opt_timeout(&Some(t), default).is_err(), "a bad spelling is refused, not defaulted");
    }

    /// The scripted responder end to end as a file: expect→send pairs render as substring case
    /// arms in declaration order, the tail is the LOUD no-match exit, and the shim is executable.
    #[test]
    fn write_shim_renders_the_scripted_responder() {
        let dir = std::env::temp_dir().join(format!("prova-term-ut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = MockCliState {
            dir: dir.clone(),
            shim: dir.join("psql"),
            pairs: vec![
                (b"SELECT 1".to_vec(), b"1 row".to_vec()),
                (b"it's".to_vec(), b"quoted".to_vec()),
            ],
        };
        write_shim(&state).unwrap();
        let script = std::fs::read_to_string(dir.join("psql")).unwrap();
        assert!(script.starts_with("#!/bin/sh"), "a POSIX responder");
        assert!(script.contains("*'SELECT 1'*) printf '%s' '1 row' ; exit 0 ;;"), "{script}");
        assert!(script.contains(r"'it'\''s'"), "embedded quotes survive: {script}");
        assert!(script.contains("no scripted expectation matched") && script.contains("exit 1"),
            "the no-match tail is loud: {script}");
        assert!(
            script.find("SELECT 1").unwrap() < script.find("it's escaped").unwrap_or(usize::MAX),
            "pairs render in declaration order"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("psql")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "executable");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scope-teardown seam's stand-in: accepts the registration so `manage` is satisfied;
    /// the test tears its children down explicitly.
    struct StubCtx;
    impl UserData for StubCtx {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("manage", |_, _, _ud: mlua::AnyUserData| Ok(()));
        }
    }

    /// The kernel PTY round-trip through the module's own Lua surface: spawn echoes through a
    /// real pty (expect observes, never sleeps), the screen model renders the frame, signal
    /// reaches the child, and wait reaps the exit code. Unix-gated like the spec suite's legs.
    #[cfg(unix)]
    #[test]
    fn spawn_drives_a_real_pty_round_trip() {
        use mlua::ObjectLike;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let lua = Lua::new();
            lua.globals().set("terminal", make(&lua).unwrap()).unwrap();
            lua.globals()
                .set("ctx", lua.create_userdata(StubCtx).unwrap())
                .unwrap();

            let refused = lua
                .load(r#"terminal.spawn(ctx, {})"#)
                .exec_async()
                .await
                .unwrap_err()
                .to_string();
            assert!(refused.contains("cmd is required"), "{refused}");

            let outcome: Table = lua
                .load(
                    r#"
                    local term = terminal.spawn(ctx, { cmd = { "cat" }, cols = 80, rows = 24 })
                    term:send("marco\r")
                    term:expect("marco", { timeout = "10s" })
                    local s = term:screen()
                    term:signal("TERM")
                    term:wait({ timeout = "10s" })

                    local exiter = terminal.spawn(ctx, { cmd = { "sh", "-c", "exit 3" } })
                    local w = exiter:wait({ timeout = "10s" })
                    return { echoed = s:contains("marco"), line = s:line(0), code = w.code }
                    "#,
                )
                .eval_async()
                .await
                .unwrap();
            assert!(outcome.get::<bool>("echoed").unwrap(), "the pty echo reached the screen");
            assert!(outcome.get::<String>("line").unwrap().contains("marco"));
            assert_eq!(outcome.get::<i64>("code").unwrap(), 3, "wait reaps the real exit code");

            // The observation layer's failure teaching: an expect that cannot be satisfied
            // reports the three facts (bytes read, reader state, child state), not just a
            // timeout — pinned here because that diagnosis once cost a forty-run bisect.
            let term: mlua::AnyUserData = lua
                .load(r#"return terminal.spawn(ctx, { cmd = { "sh", "-c", "exit 0" } })"#)
                .eval_async()
                .await
                .unwrap();
            let opts = lua.create_table().unwrap();
            opts.set("timeout", "5s").unwrap();
            let err = term
                .call_async_method::<()>("expect", ("never-appears", opts))
                .await
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("the stream ended without producing it") && err.contains("bytes read"),
                "the diagnosis names why: {err}"
            );
        });
    }
}
