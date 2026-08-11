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

use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};
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

/// Best-effort snapshot of everything still alive under a stalled child, for a failure message only.
///
/// "child still running" localizes a hang to the session but not to a process, and a pty session is
/// routinely a chain — a shell, a PATH shim, the real program under it. Which link stalled is the
/// whole question, and it is unrecoverable after the fact because teardown reaps the tree. So it is
/// captured at the moment of failure.
///
/// Failure-tolerant by construction: no `ps`, an unparsable table, or a since-exited child all
/// degrade the message and never the run. Unix-only; Windows keeps the shorter form.
#[cfg(unix)]
fn process_tree(root: Option<u32>) -> String {
    let Some(root) = root else { return String::new() };
    let Ok(out) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat=,command="])
        .output()
    else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut rows: Vec<(u32, u32, &str)> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(stat)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        rows.push((pid, ppid, line.trim()));
        let _ = stat;
    }

    // Walk down from the child: repeated sweeps, since `ps` output is not topologically ordered.
    let mut keep: Vec<u32> = vec![root];
    loop {
        let before = keep.len();
        for (pid, ppid, _) in &rows {
            if keep.contains(ppid) && !keep.contains(pid) {
                keep.push(*pid);
            }
        }
        if keep.len() == before {
            break;
        }
    }

    let listed: Vec<&str> = rows
        .iter()
        .filter(|(pid, _, _)| keep.contains(pid))
        .map(|(_, _, line)| *line)
        .collect();
    if listed.is_empty() {
        return String::new();
    }
    format!("\n-- still alive --\n{}", listed.join("\n"))
}

#[cfg(not(unix))]
fn process_tree(_root: Option<u32>) -> String {
    String::new()
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
    /// Why the reader stopped, once it has — `None` while the stream is still live.
    ///
    /// The REASON is kept, not merely the fact. A clean EOF (the child closed the pty and exited)
    /// and a failed read are the same "no more output" to a caller but completely different
    /// diagnoses when expected output never arrives: the first says the program produced nothing,
    /// the second says we may have lost what it produced. This previously collapsed to a bool, and
    /// an intermittent empty-screen failure cost a forty-run bisect that still could not tell those
    /// two apart. Cheap to carry, decisive when it matters.
    end: Option<String>,
}

impl TermBuf {
    /// The stream is finished — no further output can arrive.
    fn ended(&self) -> bool {
        self.end.is_some()
    }
}

/// Take the terminal buffer lock, recovering from poisoning: the buffer is a raw transcript plus
/// a vt parser that tolerates torn writes, so a panicked holder leaves nothing worse than a
/// truncated escape sequence — recovering beats poisoning every later read.
fn lock_buf(m: &Mutex<TermBuf>) -> std::sync::MutexGuard<'_, TermBuf> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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

/// Driving the session: `:send` raw bytes, `:expect` (observe-until-match with a timeout).
fn add_drive_methods<M: UserDataMethods<TermUd>>(methods: &mut M) {
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
                    let b = lock_buf(&this.buf);
                    if b.raw.windows(needle.len()).any(|w| w == &needle[..]) {
                        return Ok(());
                    }
                    if b.ended() {
                        let tail = String::from_utf8_lossy(&b.raw)
                            .chars()
                            .rev()
                            .take(200)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>();
                        let why = b.end.clone().unwrap_or_default();
                        let bytes = b.raw.len();
                        return Err(err(format!(
                            "expect {:?}: the stream ended without producing it \
                             [{bytes} bytes read, {why}] (transcript tail: {tail:?})",
                            String::from_utf8_lossy(&needle)
                        )));
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    // An empty screen is the least informative thing a pty failure can show,
                    // and on its own it cannot distinguish "the program never ran" from "it ran
                    // and said nothing" from "it spoke and we lost it". Report the three facts
                    // that separate those, so the first recurrence explains itself instead of
                    // needing a bisect.
                    let (screen, bytes, reader) = {
                        let b = lock_buf(&this.buf);
                        (
                            b.parser.screen().contents(),
                            b.raw.len(),
                            b.end.clone().unwrap_or_else(|| "still streaming".to_string()),
                        )
                    };
                    let child = match this.child.borrow_mut().try_wait() {
                        Ok(Some(status)) => format!("exited ({status:?})"),
                        // Alive but silent is the case worth naming precisely: it means the pty
                        // slave is still held, so output was never produced rather than lost.
                        // Which link of the chain is holding it is the actual question.
                        Ok(None) => match this.pid {
                            Some(p) => format!("still running (pid {p})"),
                            None => "still running".to_string(),
                        },
                        Err(e) => format!("status unknown ({e})"),
                    };
                    let tree = if child.starts_with("still running") {
                        process_tree(this.pid)
                    } else {
                        String::new()
                    };
                    return Err(err(format!(
                        "expect {:?}: not observed within {dur:?}\n\
                         -- pty: {bytes} bytes read, reader {reader}, child {child} --{tree}\n\
                         -- screen --\n{screen}",
                        String::from_utf8_lossy(&needle)
                    )));
                }
                tokio::time::sleep(POLL).await;
            }
        },
    );

    // Settle the frame: done when no new output for a quiet window. The anti-sleep.
}

/// Observing the frame: `:wait_stable`, `:screen()` snapshots, `:resize` (a real SIGWINCH).
fn add_observe_methods<M: UserDataMethods<TermUd>>(methods: &mut M) {
    methods.add_async_method("wait_stable", |_, this, opts: Option<Table>| async move {
        let dur = opt_timeout(&opts, DEFAULT_WAIT)?;
        let deadline = tokio::time::Instant::now() + dur;
        let mut last_len = lock_buf(&this.buf).raw.len();
        let mut quiet_since = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(POLL).await;
            let (len, ended) = {
                let b = lock_buf(&this.buf);
                (b.raw.len(), b.ended())
            };
            if len != last_len {
                last_len = len;
                quiet_since = tokio::time::Instant::now();
            } else if ended || quiet_since.elapsed() >= QUIET {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(err(format!("wait_stable: output never settled within {dur:?}")));
            }
        }
    });

    methods.add_method("screen", |lua, this, ()| {
        let b = lock_buf(&this.buf);
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
        lock_buf(&this.buf).parser.set_size(rows, cols);
        Ok(())
    });

}

/// Session lifecycle: `:signal`, `:wait` (exit status + teardown backstop).
fn add_lifecycle_methods<M: UserDataMethods<TermUd>>(methods: &mut M) {
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

impl UserData for TermUd {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_drive_methods(methods);
        add_observe_methods(methods);
        add_lifecycle_methods(methods);
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
            end: None,
        }));

        // The reader is a plain OS thread: pty reads are blocking, and this keeps the runtime
        // single-threaded. It dies at EOF (child exit / pty close on teardown).
        let thread_buf = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        lock_buf(&thread_buf).end = Some("clean EOF".to_string());
                        break;
                    }
                    // Not silently equivalent to EOF. A pty master can fail the read once the last
                    // slave closes, and whether buffered output survives that is platform-dependent
                    // — so this branch is exactly the case where the screen can come up empty
                    // through no fault of the program under test. Record what happened.
                    Err(e) => {
                        lock_buf(&thread_buf).end =
                            Some(format!("read failed: {e} ({:?})", e.kind()));
                        break;
                    }
                    Ok(n) => {
                        let mut b = lock_buf(&thread_buf);
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

    /// The screen vocabulary a proof matches colors by: the 16 ANSI names, idx-N beyond them,
    /// hex for RGB, and "default" for the terminal's own.
    #[test]
    fn color_names_speak_ansi_idx_and_rgb() {
        assert_eq!(color_name(vt100::Color::Default), "default");
        assert_eq!(color_name(vt100::Color::Idx(1)), "red");
        assert_eq!(color_name(vt100::Color::Idx(15)), "bright-white");
        assert_eq!(color_name(vt100::Color::Idx(42)), "idx-42");
        assert_eq!(color_name(vt100::Color::Rgb(255, 0, 16)), "#ff0010");
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
}
