//! The `terminal` kernel transport — PTY-backed driving of interactive programs, with a screen
//! model as the observation layer (docs/design/mocks-proxies-drivers.md, proofs/spec/terminal).
//!
//! One kernel API, not two per-OS ones: only the ALLOCATION differs by platform (openpty on unix,
//! ConPTY on Windows, both behind portable-pty). ConPTY emits the same VT sequences openpty does,
//! so the screen model — the observation layer — is byte-for-byte OS-agnostic. `terminal` is the
//! user-facing word; this pty-shaped module is the internal name.
//!
//! Driver surface: `terminal.spawn(ctx, { cmd, cols, rows, env? })` → a session with `:send`,
//! `:expect` (observe-until-match with a timeout — the same idea as `wait_for`; never a sleep),
//! `:wait_stable`, `:screen()` (→ `Screen`: text/line/cell/contains, snapshot-able), `:resize`
//! (a real SIGWINCH), `:signal`, `:wait`. Torn down via `ctx:manage` like every resource.
//!
//! Mock surface: `terminal.mock(ctx, { as = "name" })` shadows a CLI on PATH with a scripted
//! responder (expect→send pairs, generated as a self-contained POSIX shim). The narrow, true
//! mock: your SUT shells out to an interactive CLI and you script the other side.
//!
//! The pty reader is a plain OS thread feeding an `Arc<Mutex<…>>` (raw transcript + vt100 parser);
//! Lua-side waits poll it on the async runtime — no cross-thread wakers, no `Send` Lua.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{Function, Lua, ObjectLike, Table, UserData, UserDataFields, UserDataMethods, Value};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::model::parse_duration;

const DEFAULT_WAIT: Duration = Duration::from_secs(10);
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

/// Everything the reader thread produces: the raw byte transcript (what `expect` scans) and the
/// vt100 parser (what `screen()` snapshots). One lock, held briefly on both sides.
struct TermBuf {
    raw: Vec<u8>,
    parser: vt100::Parser,
    eof: bool,
}

struct TermUd {
    buf: Arc<Mutex<TermBuf>>,
    writer: RefCell<Option<Box<dyn Write + Send>>>,
    master: RefCell<Option<Box<dyn MasterPty>>>,
    child: Rc<RefCell<Box<dyn portable_pty::Child + Send + Sync>>>,
    killer: RefCell<Box<dyn ChildKiller>>,
    pid: Option<u32>,
}

/// A frozen frame — plain data copied out under the lock, so assertions never race the reader.
struct ScreenUd {
    contents: String,
    rows: u16,
    cols: u16,
    cells: Vec<Vec<CellData>>,
}

#[derive(Clone)]
struct CellData {
    ch: String,
    fg: String,
    bg: String,
    bold: bool,
}

fn color_name(c: vt100::Color) -> String {
    match c {
        vt100::Color::Default => "default".to_string(),
        vt100::Color::Idx(i) => match i {
            0 => "black".into(),
            1 => "red".into(),
            2 => "green".into(),
            3 => "yellow".into(),
            4 => "blue".into(),
            5 => "magenta".into(),
            6 => "cyan".into(),
            7 => "white".into(),
            8 => "bright-black".into(),
            9 => "bright-red".into(),
            10 => "bright-green".into(),
            11 => "bright-yellow".into(),
            12 => "bright-blue".into(),
            13 => "bright-magenta".into(),
            14 => "bright-cyan".into(),
            15 => "bright-white".into(),
            other => format!("idx-{other}"),
        },
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

impl UserData for ScreenUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("rows", |_, this| Ok(this.rows));
        fields.add_field_method_get("cols", |_, this| Ok(this.cols));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("text", |_, this, ()| Ok(this.contents.clone()));
        methods.add_method("line", |_, this, n: usize| {
            // 0-based, like cell(row, col) — screen geometry is coordinates, not Lua arrays.
            Ok(this.contents.lines().nth(n).unwrap_or("").to_string())
        });
        methods.add_method("contains", |_, this, s: String| {
            Ok(this.contents.contains(&s))
        });
        methods.add_method("cell", |lua, this, (r, c): (usize, usize)| {
            let cell = this
                .cells
                .get(r)
                .and_then(|row| row.get(c))
                .ok_or_else(|| {
                    err(format!(
                        "cell({r}, {c}): outside the {}x{} screen",
                        this.rows, this.cols
                    ))
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
        methods.add_method("snapshot_text", |_, this, ()| Ok(this.contents.clone()));
    }
}

impl UserData for TermUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("send", |_, this, data: mlua::String| {
            let mut w = this.writer.borrow_mut();
            let Some(w) = w.as_mut() else {
                return Err(err("send: session is closed"));
            };
            w.write_all(&data.as_bytes())
                .and_then(|_| w.flush())
                .map_err(|e| err(format!("send: {e}")))
        });

        // Observe-until-match with a timeout — never a sleep. Scans the raw transcript, so a
        // string that scrolled off the screen still counts as observed.
        methods.add_async_method(
            "expect",
            |_, this, (pattern, opts): (mlua::String, Option<Table>)| async move {
                let needle = pattern.as_bytes().to_vec();
                let dur = opt_timeout(&opts, DEFAULT_WAIT)?;
                let deadline = tokio::time::Instant::now() + dur;
                loop {
                    {
                        let b = this.buf.lock().unwrap();
                        if b.raw.windows(needle.len()).any(|w| w == &needle[..]) {
                            return Ok(());
                        }
                        if b.eof {
                            let tail = String::from_utf8_lossy(&b.raw)
                                .chars()
                                .rev()
                                .take(200)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect::<String>();
                            return Err(err(format!(
                                "expect {:?}: the program exited without producing it \
                                 (transcript tail: {tail:?})",
                                String::from_utf8_lossy(&needle)
                            )));
                        }
                    }
                    if tokio::time::Instant::now() >= deadline {
                        let b = this.buf.lock().unwrap();
                        let screen = b.parser.screen().contents();
                        return Err(err(format!(
                            "expect {:?}: not observed within {dur:?}\n-- screen --\n{screen}",
                            String::from_utf8_lossy(&needle)
                        )));
                    }
                    tokio::time::sleep(POLL).await;
                }
            },
        );

        // Settle the frame: done when no new output for a quiet window. The anti-sleep.
        methods.add_async_method("wait_stable", |_, this, opts: Option<Table>| async move {
            let dur = opt_timeout(&opts, DEFAULT_WAIT)?;
            let deadline = tokio::time::Instant::now() + dur;
            let mut last_len = this.buf.lock().unwrap().raw.len();
            let mut quiet_since = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(POLL).await;
                let (len, eof) = {
                    let b = this.buf.lock().unwrap();
                    (b.raw.len(), b.eof)
                };
                if len != last_len {
                    last_len = len;
                    quiet_since = tokio::time::Instant::now();
                } else if eof || quiet_since.elapsed() >= QUIET {
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(err(format!("wait_stable: output never settled within {dur:?}")));
                }
            }
        });

        methods.add_method("screen", |lua, this, ()| {
            let b = this.buf.lock().unwrap();
            let screen = b.parser.screen();
            let (rows, cols) = screen.size();
            let mut cells = Vec::with_capacity(rows as usize);
            for r in 0..rows {
                let mut row = Vec::with_capacity(cols as usize);
                for c in 0..cols {
                    let cell = screen.cell(r, c);
                    row.push(match cell {
                        Some(cl) => CellData {
                            ch: cl.contents(),
                            fg: color_name(cl.fgcolor()),
                            bg: color_name(cl.bgcolor()),
                            bold: cl.bold(),
                        },
                        None => CellData {
                            ch: String::new(),
                            fg: "default".into(),
                            bg: "default".into(),
                            bold: false,
                        },
                    });
                }
                cells.push(row);
            }
            lua.create_userdata(ScreenUd {
                contents: screen.contents(),
                rows,
                cols,
                cells,
            })
        });

        // A real SIGWINCH: the pty is resized, the child is signaled, and the parser's geometry
        // follows — `stty size` inside the session reports the new numbers.
        methods.add_method("resize", |_, this, (cols, rows): (u16, u16)| {
            let m = this.master.borrow();
            let Some(m) = m.as_ref() else {
                return Err(err("resize: session is closed"));
            };
            m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| err(format!("resize: {e}")))?;
            this.buf.lock().unwrap().parser.set_size(rows, cols);
            Ok(())
        });

        methods.add_method("signal", |_, this, name: String| {
            #[cfg(unix)]
            {
                let sig = match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
                    "INT" => libc::SIGINT,
                    "TERM" => libc::SIGTERM,
                    "KILL" => libc::SIGKILL,
                    "HUP" => libc::SIGHUP,
                    "QUIT" => libc::SIGQUIT,
                    "USR1" => libc::SIGUSR1,
                    "USR2" => libc::SIGUSR2,
                    "WINCH" => libc::SIGWINCH,
                    other => return Err(err(format!("signal: unknown signal {other:?}"))),
                };
                let Some(pid) = this.pid else {
                    return Err(err("signal: child pid unknown"));
                };
                let r = unsafe { libc::kill(pid as libc::pid_t, sig) };
                if r != 0 {
                    return Err(err(format!(
                        "signal {name}: kill({pid}) failed: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                Ok(())
            }
            #[cfg(not(unix))]
            {
                let _ = name;
                // Annotate the Ok type: this branch has no `Ok(())` to pin it, so `Err(..)` alone
                // leaves the success type ambiguous (E0283) — only surfaces on non-unix builds.
                Err::<(), _>(err(
                    "signal: POSIX signals need a unix platform (ConPTY has no signal channel)",
                ))
            }
        });

        // Reap the child and report its exit code. Polling try_wait keeps everything on the
        // single-threaded runtime — no blocking wait, no Send bound on the child handle.
        methods.add_async_method("wait", |lua, this, opts: Option<Table>| async move {
            let dur = opt_timeout(&opts, Duration::from_secs(30))?;
            let deadline = tokio::time::Instant::now() + dur;
            loop {
                let status = this
                    .child
                    .borrow_mut()
                    .try_wait()
                    .map_err(|e| err(format!("wait: {e}")))?;
                if let Some(s) = status {
                    let t = lua.create_table()?;
                    t.set("code", s.exit_code())?;
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
            let _ = this.killer.borrow_mut().kill();
            this.writer.borrow_mut().take();
            this.master.borrow_mut().take();
            Ok(())
        });
    }
}

fn spawn_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
        let cmd: Vec<String> = opts
            .get::<Option<Vec<String>>>("cmd")?
            .filter(|v| !v.is_empty())
            .ok_or_else(|| err("terminal.spawn(ctx, { cmd = { … } }): cmd is required"))?;
        let cols = opts.get::<Option<u16>>("cols")?.unwrap_or(80);
        let rows = opts.get::<Option<u16>>("rows")?.unwrap_or(24);

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| err(format!("terminal.spawn: openpty: {e}")))?;

        let mut builder = CommandBuilder::new(&cmd[0]);
        builder.args(&cmd[1..]);
        if let Some(cwd) = opts.get::<Option<String>>("cwd")? {
            builder.cwd(cwd);
        }
        if let Some(env) = opts.get::<Option<Table>>("env")? {
            for pair in env.pairs::<String, String>() {
                let (k, v) = pair?;
                builder.env(k, v);
            }
        }
        // A plain terminal identity so programs emit the classic VT sequences vt100 parses.
        builder.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| err(format!("terminal.spawn {:?}: {e}", cmd[0])))?;
        drop(pair.slave);

        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| err(format!("terminal.spawn: reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| err(format!("terminal.spawn: writer: {e}")))?;

        let buf = Arc::new(Mutex::new(TermBuf {
            raw: Vec::new(),
            parser: vt100::Parser::new(rows, cols, 0),
            eof: false,
        }));

        // The reader is a plain OS thread: pty reads are blocking, and this keeps the runtime
        // single-threaded. It dies at EOF (child exit / pty close on teardown).
        let thread_buf = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => {
                        thread_buf.lock().unwrap().eof = true;
                        break;
                    }
                    Ok(n) => {
                        let mut b = thread_buf.lock().unwrap();
                        b.raw.extend_from_slice(&chunk[..n]);
                        b.parser.process(&chunk[..n]);
                    }
                }
            }
        });

        let ud = lua.create_userdata(TermUd {
            buf,
            writer: RefCell::new(Some(writer)),
            master: RefCell::new(Some(pair.master)),
            child: Rc::new(RefCell::new(child)),
            killer: RefCell::new(killer),
            pid,
        })?;
        manage("terminal.spawn", &ctx, &ud)?;
        Ok(ud)
    })
}

fn manage(what: &str, ctx: &Value, ud: &mlua::AnyUserData) -> mlua::Result<()> {
    match ctx {
        Value::UserData(c) => {
            let _: Value = c.call_method("manage", ud)?;
            Ok(())
        }
        Value::Nil => Err(err(format!(
            "{what}(ctx): pass the test or fixture context (`t` / `ctx`) so it is torn down with \
             the scope"
        ))),
        other => Err(err(format!(
            "{what}(ctx): expected the test or fixture context, got a {}",
            other.type_name()
        ))),
    }
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

fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
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
            &name
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
        manage("terminal.mock", &ctx, &ud)?;
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

impl UserData for TermProxyUd {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("env", |lua, this| lua.registry_value::<Table>(&this.env));
        fields.add_field_method_get("path", |_, this| {
            Ok(this.state.borrow().shim.to_string_lossy().to_string())
        });
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

fn proxy_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (ctx, opts): (Value, Table)| {
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
            &name
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
            let text = std::fs::read_to_string(cassette.as_ref().unwrap())
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
        manage("terminal.proxy", &ctx, &ud)?;
        Ok(ud)
    })
}
