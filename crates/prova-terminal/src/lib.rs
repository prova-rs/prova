//! The terminal kernel — a program driven through a real PTY, observed through a VT screen model.
//!
//! One API, not two per-OS ones: only the ALLOCATION differs by platform (openpty on unix, ConPTY
//! on Windows, both behind portable-pty). ConPTY emits the same VT sequences openpty does, so the
//! screen model — the observation layer — is byte-for-byte OS-agnostic.
//!
//! The kernel owns the SESSION and no policy. It has no Lua, no async runtime and no clock: every
//! wait is a non-blocking CHECK ([`Session::check_expect`], [`Session::activity`]) that a host
//! loops over and bounds as it sees fit — prova's `terminal` driver by deadline, an agent host by
//! progress. The reader is a plain OS thread feeding one `Arc<Mutex<…>>` (raw transcript + vt100
//! parser), so a host needs no cross-thread wakers and no `Send` on its own side.
//!
//! Extracted from prova-core's `terminal` module with no change in behaviour
//! (docs/design/terminal-kernel.md).

use std::fmt;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

mod shadow;
use shadow::{AttrShadow, ColorNormalizer};

/// What to run, and on how large a terminal.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// The program and its arguments; the first element is the program.
    pub cmd: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    /// The child's working directory; the host's own when `None`.
    pub cwd: Option<String>,
    /// Extra environment, applied in order. `TERM` is always `xterm-256color` (set last), so a
    /// program emits the classic VT sequences the screen model parses.
    pub env: Vec<(String, String)>,
}

impl SpawnSpec {
    /// An 80x24 terminal running `cmd`, in the host's directory and environment.
    pub fn new(cmd: Vec<String>) -> Self {
        Self { cmd, cols: 80, rows: 24, cwd: None, env: Vec::new() }
    }
}

/// Why a kernel call failed. `Display` is the cause alone; the host names the verb.
#[derive(Debug)]
pub enum Error {
    /// The spec named no program.
    NoCommand,
    /// The PTY could not be allocated.
    Openpty(String),
    /// The program could not be started in the PTY.
    Spawn { program: String, cause: String },
    /// The PTY master's reader could not be cloned.
    Reader(String),
    /// The PTY master's writer could not be taken.
    Writer(String),
    /// The session's PTY is closed — [`Session::stop`] ran.
    Closed,
    /// A write, flush or resize on the PTY failed.
    Io(String),
    /// [`Session::signal`] was given a name it does not deliver.
    UnknownSignal(String),
    /// The child's pid is unknown, so no signal can reach it.
    NoPid,
    /// `kill(2)` refused the signal.
    Kill { signal: String, pid: u32, cause: String },
    /// This platform has no POSIX signals (ConPTY has no signal channel).
    SignalsUnsupported,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoCommand => write!(f, "cmd is required"),
            Error::Openpty(c) => write!(f, "openpty: {c}"),
            Error::Spawn { program, cause } => write!(f, "{program:?}: {cause}"),
            Error::Reader(c) => write!(f, "reader: {c}"),
            Error::Writer(c) => write!(f, "writer: {c}"),
            Error::Closed => write!(f, "session is closed"),
            Error::Io(c) => write!(f, "{c}"),
            Error::UnknownSignal(name) => write!(f, "unknown signal {name:?}"),
            Error::NoPid => write!(f, "child pid unknown"),
            Error::Kill { signal, pid, cause } => write!(f, "{signal}: kill({pid}) failed: {cause}"),
            Error::SignalsUnsupported => {
                write!(f, "POSIX signals need a unix platform (ConPTY has no signal channel)")
            }
        }
    }
}

impl std::error::Error for Error {}

/// What vt100 hands back instead of handling: the cursor style the program asked for (DECSCUSR),
/// its window title, and the queries a real terminal answers. A reply is QUEUED here and written by
/// the reader thread after the chunk is parsed — the callback runs under the buffer lock and must
/// never block on the pty.
///
/// The replies are termlens's, byte for byte (MIT OR Apache-2.0, github.com/vyncint/termlens,
/// `Terminal::answer`): a VT220 with ANSI colour and nothing it cannot render. The oracle holds
/// them to that.
#[derive(Default)]
struct Tracker {
    /// The last DECSCUSR parameter (0..=6); `None` while the program never asked.
    cursor_style: Option<u16>,
    title: String,
    replies: Vec<u8>,
}

impl vt100::Callbacks for Tracker {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        // An absent first parameter reads as 0, as the VT spec defaults it.
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (i1, i2, c) {
            // DECSCUSR — `CSI Ps SP q`. A value the spec does not define is ignored, never guessed.
            (Some(b' '), None, 'q') => {
                if first <= 6 {
                    self.cursor_style = Some(first);
                }
                return;
            }
            // Primary device attributes: a VT220 with ANSI colour.
            (None, None, 'c') if first == 0 => b"\x1b[?62;22c".to_vec(),
            // Secondary device attributes.
            (Some(b'>'), None, 'c') if first == 0 => b"\x1b[>1;10;0c".to_vec(),
            // Device status: "OK".
            (None, None, 'n') if first == 5 => b"\x1b[0n".to_vec(),
            // Cursor position report (DSR 6, and DECXCPR with `?`), 1-based on the wire.
            (None | Some(b'?'), None, 'n') if first == 6 => {
                let (row, col) = screen.cursor_position();
                let private = if i1 == Some(b'?') { "?" } else { "" };
                format!("\x1b[{private}{};{}R", row + 1, col + 1).into_bytes()
            }
            // Text-area size in characters.
            (None, None, 't') if first == 18 => {
                let (rows, cols) = screen.size();
                format!("\x1b[8;{rows};{cols}t").into_bytes()
            }
            _ => return,
        };
        self.replies.extend_from_slice(&reply);
    }
}

/// Everything the reader thread produces: the raw byte transcript (what [`Session::check_expect`
/// scans) and the vt100 parser (what [`Session::screen`] snapshots). One lock, held briefly on
/// both sides.
struct TermBuf {
    raw: Vec<u8>,
    /// Colon-form colours (`38:2::r:g:b`) rewritten to the form vt100 reads, before either parser.
    normalizer: ColorNormalizer,
    parser: vt100::Parser<Tracker>,
    /// The parallel parser carrying blink, conceal and strikethrough (`shadow.rs`).
    shadow: AttrShadow,
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

/// Take the pty writer lock, recovering from poisoning: a writer is a byte sink, and a panicked
/// holder leaves at worst a partial write — recovering beats refusing every later send.
fn lock_writer(
    m: &Mutex<Option<Box<dyn Write + Send>>>,
) -> MutexGuard<'_, Option<Box<dyn Write + Send>>> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Take the terminal buffer lock, recovering from poisoning: the buffer is a raw transcript plus
/// a vt parser that tolerates torn writes, so a panicked holder leaves nothing worse than a
/// truncated escape sequence — recovering beats poisoning every later read.
fn lock_buf(m: &Mutex<TermBuf>) -> MutexGuard<'_, TermBuf> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A program running in a PTY. `Send`, so a host may keep it on any thread; every method is
/// non-blocking except [`Session::spawn`].
pub struct Session {
    buf: Arc<Mutex<TermBuf>>,
    /// Shared with the reader thread, which writes the query replies through it; `None` once
    /// [`Session::stop`] closed the session, which silences the replies too.
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pid: Option<u32>,
}

/// One non-blocking look for a byte string in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectCheck {
    /// The transcript holds it — including output that has since scrolled off the screen.
    Found,
    /// Not yet, and the stream is still live.
    Pending,
    /// The stream ended without producing it.
    Ended {
        /// Bytes read over the session's life.
        bytes: usize,
        /// Why the reader stopped ("clean EOF", or the failed read).
        why: String,
        /// The transcript's last 200 characters.
        tail: String,
    },
}

/// How a reaped child exited. The kernel's own type, so portable-pty stays out of the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The exit code.
    pub code: u32,
}

/// How much output the session has produced so far, and whether it can produce more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub bytes: usize,
    pub ended: bool,
}

/// The facts that separate "the program never ran" from "it ran and said nothing" from "it spoke
/// and we lost it" — what a host reports when a wait gives up. An empty screen alone cannot tell
/// those apart.
#[derive(Debug, Clone)]
pub struct Stall {
    /// The frame's text at the moment of the stall.
    pub screen: String,
    /// Bytes read over the session's life.
    pub bytes: usize,
    /// The reader's state: its end reason, or "still streaming".
    pub reader: String,
    /// The child's state: "exited (…)", "still running (pid N)", or "status unknown (…)".
    pub child: String,
    /// While the child still runs, everything alive under it (`\n-- still alive --\n…`); else
    /// empty.
    pub tree: String,
}

impl Session {
    /// Allocate a PTY of the spec's size, start the program in it, and start the reader thread.
    pub fn spawn(spec: &SpawnSpec) -> Result<Session, Error> {
        let program = spec.cmd.first().ok_or(Error::NoCommand)?;
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: spec.rows, cols: spec.cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| Error::Openpty(e.to_string()))?;

        let mut builder = CommandBuilder::new(program);
        builder.args(&spec.cmd[1..]);
        if let Some(cwd) = &spec.cwd {
            builder.cwd(cwd);
        }
        for (k, v) in &spec.env {
            builder.env(k, v);
        }
        // A plain terminal identity so programs emit the classic VT sequences vt100 parses.
        builder.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| Error::Spawn { program: program.clone(), cause: e.to_string() })?;
        drop(pair.slave);

        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(|e| Error::Reader(e.to_string()))?;
        let writer = pair.master.take_writer().map_err(|e| Error::Writer(e.to_string()))?;

        let buf = Arc::new(Mutex::new(TermBuf {
            raw: Vec::new(),
            normalizer: ColorNormalizer::new(),
            parser: vt100::Parser::new_with_callbacks(spec.rows, spec.cols, 0, Tracker::default()),
            shadow: AttrShadow::new(spec.rows, spec.cols),
            end: None,
        }));
        let writer = Arc::new(Mutex::new(Some(writer)));

        // The reader is a plain OS thread: pty reads are blocking, and this keeps every host's
        // runtime its own business. It dies at EOF (child exit / pty close on teardown).
        let thread_buf = buf.clone();
        let thread_writer = writer.clone();
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
                        lock_buf(&thread_buf).end = Some(format!("read failed: {e} ({:?})", e.kind()));
                        break;
                    }
                    Ok(n) => {
                        let replies = {
                            let mut b = lock_buf(&thread_buf);
                            b.raw.extend_from_slice(&chunk[..n]);
                            let bytes = b.normalizer.feed(&chunk[..n]);
                            b.parser.process(&bytes);
                            b.shadow.feed(&bytes);
                            std::mem::take(&mut b.parser.callbacks_mut().replies)
                        };
                        // Answer outside the buffer lock. A failed write leaves the program
                        // unanswered — exactly what it saw before there was a responder — so
                        // there is nothing to report, and the next read carries on.
                        if !replies.is_empty() {
                            if let Some(w) = lock_writer(&thread_writer).as_mut() {
                                let _ = w.write_all(&replies).and_then(|()| w.flush());
                            }
                        }
                    }
                }
            }
        });

        Ok(Session { buf, writer, master: Some(pair.master), child, killer, pid })
    }

    /// The child's pid, when the platform reports one.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Write raw bytes to the program's input and flush them.
    pub fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        let mut guard = lock_writer(&self.writer);
        let w = guard.as_mut().ok_or(Error::Closed)?;
        w.write_all(data).and_then(|()| w.flush()).map_err(|e| Error::Io(e.to_string()))
    }

    /// Look once for `needle` in the raw transcript, so a string that scrolled off the screen
    /// still counts as observed.
    pub fn check_expect(&self, needle: &[u8]) -> ExpectCheck {
        let b = lock_buf(&self.buf);
        if b.raw.windows(needle.len()).any(|w| w == needle) {
            return ExpectCheck::Found;
        }
        if !b.ended() {
            return ExpectCheck::Pending;
        }
        let tail = String::from_utf8_lossy(&b.raw)
            .chars()
            .rev()
            .take(200)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        ExpectCheck::Ended { bytes: b.raw.len(), why: b.end.clone().unwrap_or_default(), tail }
    }

    /// Bytes produced so far, and whether the stream has ended.
    pub fn activity(&self) -> Activity {
        let b = lock_buf(&self.buf);
        Activity { bytes: b.raw.len(), ended: b.ended() }
    }

    /// Freeze the current frame: plain data copied out under the lock, so assertions never race
    /// the reader.
    pub fn screen(&self) -> Screen {
        let b = lock_buf(&self.buf);
        let screen = b.parser.screen();
        let (rows, cols) = screen.size();
        // Attributes never shape vt100's grid, so the shadow's text is the primary's: shadow cell
        // (r, c) IS primary cell (r, c). Checked, not argued.
        debug_assert_eq!(screen.contents(), b.shadow.contents(), "the attribute shadow drifted");
        let mut cells = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols as usize);
            for c in 0..cols {
                let carried = b.shadow.cell(r, c);
                row.push(match screen.cell(r, c) {
                    Some(cl) => Cell {
                        ch: cl.contents().to_string(),
                        fg: color_name(cl.fgcolor()),
                        bg: color_name(cl.bgcolor()),
                        bold: cl.bold(),
                        dim: cl.dim(),
                        italic: cl.italic(),
                        underline: cl.underline(),
                        reverse: cl.inverse(),
                        blink: carried.is_some_and(vt100::Cell::bold),
                        conceal: carried.is_some_and(vt100::Cell::italic),
                        strikethrough: carried.is_some_and(vt100::Cell::underline),
                    },
                    None => Cell {
                        ch: String::new(),
                        fg: "default".into(),
                        bg: "default".into(),
                        bold: false,
                        dim: false,
                        italic: false,
                        underline: false,
                        reverse: false,
                        blink: false,
                        conceal: false,
                        strikethrough: false,
                    },
                });
            }
            cells.push(row);
        }
        let lines = screen.rows(0, cols).map(|l| l.trim_end().to_string()).collect();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let tracker = b.parser.callbacks();
        Screen {
            contents: screen.contents(),
            lines,
            rows,
            cols,
            cells,
            cursor: (cursor_row, cursor_col),
            cursor_visible: !screen.hide_cursor(),
            cursor_shape: match tracker.cursor_style {
                None => CursorShape::Default,
                Some(0..=2) => CursorShape::Block,
                Some(3..=4) => CursorShape::Underline,
                Some(_) => CursorShape::Bar,
            },
            cursor_blink: tracker.cursor_style.map(|s| matches!(s, 0 | 1 | 3 | 5)),
            title: tracker.title.clone(),
            alternate_screen: screen.alternate_screen(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_reporting: screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
        }
    }

    /// A real SIGWINCH: the pty is resized, the child is signaled, and the screen model's
    /// geometry follows — `stty size` inside the session reports the new numbers.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Error> {
        let m = self.master.as_ref().ok_or(Error::Closed)?;
        m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| Error::Io(e.to_string()))?;
        let mut b = lock_buf(&self.buf);
        b.parser.screen_mut().set_size(rows, cols);
        b.shadow.set_size(rows, cols);
        Ok(())
    }

    /// Deliver a POSIX signal to the child by name — `INT`, `TERM`, `KILL`, `HUP`, `QUIT`,
    /// `USR1`, `USR2` or `WINCH`, with or without the `SIG` prefix, in any case.
    #[cfg(unix)]
    pub fn signal(&self, name: &str) -> Result<(), Error> {
        let sig = match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
            "INT" => libc::SIGINT,
            "TERM" => libc::SIGTERM,
            "KILL" => libc::SIGKILL,
            "HUP" => libc::SIGHUP,
            "QUIT" => libc::SIGQUIT,
            "USR1" => libc::SIGUSR1,
            "USR2" => libc::SIGUSR2,
            "WINCH" => libc::SIGWINCH,
            other => return Err(Error::UnknownSignal(other.to_string())),
        };
        let pid = self.pid.ok_or(Error::NoPid)?;
        // SAFETY: kill(2) with a pid this session spawned and a valid signal number; it touches
        // no memory, and a stale pid fails with ESRCH, which is reported.
        let r = unsafe { libc::kill(pid as libc::pid_t, sig) };
        if r != 0 {
            return Err(Error::Kill {
                signal: name.to_string(),
                pid,
                cause: std::io::Error::last_os_error().to_string(),
            });
        }
        Ok(())
    }

    /// POSIX signals need a unix platform: ConPTY has no signal channel.
    #[cfg(not(unix))]
    pub fn signal(&self, _name: &str) -> Result<(), Error> {
        Err(Error::SignalsUnsupported)
    }

    /// Reap the child if it has exited; `None` while it runs. Never blocks.
    pub fn try_wait(&mut self) -> std::io::Result<Option<Exit>> {
        Ok(self.child.try_wait()?.map(|s| Exit { code: s.exit_code() }))
    }

    /// The facts a host reports when a wait gives up (see [`Stall`]).
    pub fn diagnose(&mut self) -> Stall {
        let (screen, bytes, reader) = {
            let b = lock_buf(&self.buf);
            (
                b.parser.screen().contents(),
                b.raw.len(),
                b.end.clone().unwrap_or_else(|| "still streaming".to_string()),
            )
        };
        let child = match self.child.try_wait() {
            Ok(Some(status)) => format!("exited ({status:?})"),
            // Alive but silent is the case worth naming precisely: it means the pty slave is still
            // held, so output was never produced rather than lost. Which link of the chain is
            // holding it is the actual question.
            Ok(None) => match self.pid {
                Some(p) => format!("still running (pid {p})"),
                None => "still running".to_string(),
            },
            Err(e) => format!("status unknown ({e})"),
        };
        let tree = if child.starts_with("still running") { process_tree(self.pid) } else { String::new() };
        Stall { screen, bytes, reader, child, tree }
    }

    /// Kill the child and close the pty. Idempotent; a kill that fails because the child already
    /// exited is the expected case, not an error.
    pub fn stop(&mut self) {
        // A child that already exited refuses the kill; either way nothing is left running.
        let _ = self.killer.kill();
        lock_writer(&self.writer).take();
        self.master.take();
    }
}

/// A frozen frame — plain data, safe to hold while the program keeps writing.
#[derive(Debug, Clone)]
pub struct Screen {
    /// The frame as LOGICAL text: a soft-wrapped row continues its line, hard line breaks separate
    /// lines, trailing blanks are trimmed. What `contains` searches, so a word the terminal wrapped
    /// is still found.
    pub contents: String,
    /// Each GRID row's text, trailing blanks trimmed — `lines[r]` is row `r` of `cells`.
    pub lines: Vec<String>,
    pub rows: u16,
    pub cols: u16,
    /// `cells[row][col]`, 0-based.
    pub cells: Vec<Vec<Cell>>,
    /// The cursor, `(row, col)`, 0-based.
    pub cursor: (u16, u16),
    /// Whether the cursor is shown (`DECTCEM`).
    pub cursor_visible: bool,
    /// The cursor the program asked for with DECSCUSR — [`CursorShape::Default`] while it never did.
    pub cursor_shape: CursorShape,
    /// Whether that cursor blinks; `None` while the program never said (its default is the
    /// terminal's, not ours to claim).
    pub cursor_blink: Option<bool>,
    /// The window title the program set (`OSC 0` / `OSC 2`); empty when it never did.
    pub title: String,
    /// Whether the program is on the alternate screen (`?1049` and kin).
    pub alternate_screen: bool,
    /// Application cursor keys (`DECCKM`, `?1`): arrows arrive as `ESC O x`, not `ESC [ x`.
    pub application_cursor: bool,
    /// Bracketed paste (`?2004`): a paste arrives fenced by `ESC [200~` / `ESC [201~`.
    pub bracketed_paste: bool,
    /// Whether the program asked for any mouse reporting.
    pub mouse_reporting: bool,
}

impl Screen {
    /// GRID row `n`'s text (0-based), trailing blanks trimmed; empty past the last row. A row, not
    /// a logical line: a soft-wrapped line spans several of these, as it spans several rows of
    /// `cells`. (Until 2026-09-19 this read the logical text, so a wrapped row read as the whole
    /// line and disagreed with `cell` — the oracle's `wrap/text`.)
    pub fn line(&self, n: usize) -> &str {
        self.lines.get(n).map_or("", String::as_str)
    }

    pub fn contains(&self, s: &str) -> bool {
        self.contents.contains(s)
    }

    /// The cell at `(row, col)`, 0-based; `None` outside the screen.
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.cells.get(row).and_then(|r| r.get(col))
    }
}

/// The cursor a program asked for with DECSCUSR (`CSI Ps SP q`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// Never asked: whatever the terminal draws by default. Not folded into `Block` — "never
    /// asked" and "asked for a block" are different claims about the program.
    #[default]
    Default,
    /// DECSCUSR 0, 1 (blinking) or 2 (steady).
    Block,
    /// DECSCUSR 3 (blinking) or 4 (steady).
    Underline,
    /// DECSCUSR 5 (blinking) or 6 (steady).
    Bar,
}

/// One screen cell: its character and its rendition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The cell's contents — empty for a never-written cell.
    pub ch: String,
    /// Foreground colour: one of the 16 ANSI names (`red`, `bright-white`, …), `idx-N` beyond
    /// them, `#rrggbb` for RGB, or `default` for the terminal's own.
    pub fg: String,
    /// Background colour, in the same vocabulary as `fg`.
    pub bg: String,
    pub bold: bool,
    /// Decreased intensity (`SGR 2`); shares one intensity state with `bold` — last write wins.
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    /// Reverse video (`SGR 7`): foreground and background swapped.
    pub reverse: bool,
    /// Blinking (`SGR 5`/`6`; the two rates are not distinguished).
    pub blink: bool,
    /// Concealed (`SGR 8`): the cell holds text the terminal does not display — a masked password
    /// field. `ch` still reports the text, as a real terminal holds it; this says it is hidden.
    pub conceal: bool,
    /// Struck through (`SGR 9`).
    pub strikethrough: bool,
}

/// The colour vocabulary a proof matches by: the 16 ANSI names, `idx-N` beyond them, `#rrggbb`
/// for RGB, and `default` for the terminal's own. Private, so vt100 stays out of the public API.
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

/// Best-effort snapshot of everything still alive under a stalled child, for a failure message
/// only.
///
/// "child still running" localizes a hang to the session but not to a process, and a pty session
/// is routinely a chain — a shell, a PATH shim, the real program under it. Which link stalled is
/// the whole question, and it is unrecoverable after the fact because teardown reaps the tree. So
/// it is captured at the moment of failure.
///
/// Failure-tolerant by construction: no `ps`, an unparsable table, or a since-exited child all
/// degrade the message and never the run. Unix-only; Windows keeps the shorter form.
#[cfg(unix)]
fn process_tree(root: Option<u32>) -> String {
    let Some(root) = root else { return String::new() };
    let Ok(out) = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,stat=,command="]).output()
    else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut rows: Vec<(u32, u32, &str)> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (it.next(), it.next()) else {
            continue;
        };
        // A process row names its state before its command; a row without one is not a process.
        if it.next().is_none() {
            continue;
        }
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        rows.push((pid, ppid, line.trim()));
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

    let listed: Vec<&str> =
        rows.iter().filter(|(pid, _, _)| keep.contains(pid)).map(|(_, _, line)| *line).collect();
    if listed.is_empty() {
        return String::new();
    }
    format!("\n-- still alive --\n{}", listed.join("\n"))
}

/// Unix-only: Windows keeps the shorter failure message.
#[cfg(not(unix))]
fn process_tree(_root: Option<u32>) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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

    #[test]
    fn an_empty_command_is_refused() {
        let err = Session::spawn(&SpawnSpec::new(Vec::new())).err().expect("refused");
        assert!(matches!(err, Error::NoCommand), "{err}");
    }

    /// Loop a check the way a host does — the kernel never sleeps, so the test owns the bound.
    fn until<T>(bound: Duration, mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + bound;
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "not observed within {bound:?}");
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    /// The kernel round trip with no host at all: the pty echo reaches the transcript and the
    /// frozen frame, a signal reaches the child, try_wait reaps the real exit code, and a stream
    /// that ends without the needle says so with the facts that explain it.
    #[cfg(unix)]
    #[test]
    fn a_real_pty_round_trip() {
        let bound = Duration::from_secs(10);
        let mut s = Session::spawn(&SpawnSpec::new(vec!["cat".into()])).expect("spawn cat");
        s.send(b"marco\r").expect("send");
        until(bound, || (s.check_expect(b"marco") == ExpectCheck::Found).then_some(()));
        let frame = s.screen();
        assert!(frame.contains("marco") && frame.line(0).contains("marco"), "{}", frame.contents);
        assert_eq!((frame.rows, frame.cols), (24, 80));
        assert_eq!(frame.cell(0, 0).map(|c| c.ch.as_str()), Some("m"));
        assert!(frame.cell(24, 0).is_none(), "outside the screen");
        s.signal("TERM").expect("signal");
        until(bound, || s.try_wait().expect("try_wait"));

        let mut exiter = Session::spawn(&SpawnSpec::new(vec!["sh".into(), "-c".into(), "exit 3".into()]))
            .expect("spawn sh");
        let status = until(bound, || exiter.try_wait().expect("try_wait"));
        assert_eq!(status.code, 3, "the real exit code");

        let quiet = Session::spawn(&SpawnSpec::new(vec!["sh".into(), "-c".into(), "exit 0".into()]))
            .expect("spawn sh");
        let ended = until(bound, || match quiet.check_expect(b"never-appears") {
            ExpectCheck::Pending => None,
            other => Some(other),
        });
        match ended {
            ExpectCheck::Ended { why, .. } => assert!(!why.is_empty(), "the reader's end reason is kept"),
            other => panic!("expected the stream to end, got {other:?}"),
        }
        assert!(quiet.activity().ended);
    }

    /// Stop is idempotent and closes the session: later sends and resizes refuse as Closed.
    #[cfg(unix)]
    #[test]
    fn stop_closes_the_session() {
        let mut s = Session::spawn(&SpawnSpec::new(vec!["cat".into()])).expect("spawn cat");
        s.stop();
        s.stop();
        assert!(matches!(s.send(b"x"), Err(Error::Closed)));
        assert!(matches!(s.resize(100, 30), Err(Error::Closed)));
    }

    #[cfg(unix)]
    #[test]
    fn an_unknown_signal_is_refused_by_name() {
        let s = Session::spawn(&SpawnSpec::new(vec!["cat".into()])).expect("spawn cat");
        let err = s.signal("BOGUS").expect_err("refused");
        assert_eq!(err.to_string(), "unknown signal \"BOGUS\"");
    }
}
