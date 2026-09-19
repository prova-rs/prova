# The terminal kernel

**Status:** slice 1 landed as an extraction with no change in behaviour (2026-09-19). The crate is
`prova-terminal` (`crates/prova-terminal`). It is publishable (crates.io dependencies only, no pty
type in its public API) but not yet published: a host outside this workspace pins it by git rev and
watches prova's `main` with a reminder (ruling 2026-09-19).

## Why a kernel

Three copies of one idea existed — prova's `terminal` driver, Substrate's `cos.process.pty_*` and
termlens — all on portable-pty + vt100, and none complete. The ruling (Substrate landmark
01a0b8bf-a869, 2026-09-19): ONE terminal kernel that lives in prova, with two faces over it.
- **prova's face** is the `terminal` module. It is a test driver: deadline-bounded waits,
  `ctx:manage` teardown, mocks and proxies.
- **Substrate's face** is `cos.process.pty_*`. It is an agent surface: waits bounded by progress,
  runs visible to the process census, and every refusal carrying the screen.

termlens (MIT OR Apache-2.0) is a source of ideas and portable code and the kernel's dev-only
**oracle**, never a runtime dependency. Its `Terminal` owns the spawn, the reader and the child,
which a supervisor that must own the child cannot accept.

## The seam

The kernel owns the **session** and no **policy**.

| Kernel (`prova-terminal`) | Face (prova's `terminal` module) |
|---|---|
| `Session::spawn(&SpawnSpec)`: PTY allocation, `TERM=xterm-256color`, the child, the reader thread | `terminal.spawn(ctx, {cmd, cols, rows, cwd, env})`: argument shape, `ctx:manage` |
| `send`, `resize`, `signal`, `try_wait`, `stop` | `:send`, `:resize`, `:signal`, `:wait` (a 30 s deadline), `:stop` |
| `check_expect(needle)` → `Found` / `Pending` / `Ended{bytes, why, tail}` | `:expect`: the loop, the 15 ms poll, the `timeout` deadline, the message |
| `activity()` → `{bytes, ended}` | `:wait_stable`: the 150 ms quiet window, the deadline |
| `screen()` → a frozen `Screen` (logical `contents`, grid `lines`, `cell` → `Cell{ch, fg, bg, bold, dim, italic, underline, reverse}`, `cursor`, `cursor_visible`, `cursor_shape`, `cursor_blink`, `title`, `alternate_screen`, `application_cursor`, `bracketed_paste`, `mouse_reporting`) | `Screen` userdata (`text`, `line`, `contains`, `cell`), `snapshot_text` for `matches_snapshot` |
| the **query responder**: DA1, DA2, DSR 5, CPR / DECXCPR and the text-area size, each reply termlens's byte for byte, queued by vt100's callbacks and written by the reader thread outside the buffer lock | nothing to configure: a program that probes its terminal gets an answer |
| `diagnose()` → `Stall{screen, bytes, reader, child, tree}`, with `process_tree` under a live child | the timeout message that reports it |

**Every wait in the kernel is a non-blocking check.** There is no clock, no sleep and no async
runtime in the crate, so each host bounds its waits its own way. A deadline is right for a test.
For an agent, the right bound is progress (CLAUDE.md in Substrate: "bound the resource, never the
work"). The reader is a plain OS thread behind one `Arc<Mutex<…>>` (transcript + parser), so a host
needs no wakers and no `Send` on its own side. `Session` is itself `Send`.

**Errors carry the cause, not the verb.** `Error`'s `Display` is the cause alone. The face names
the verb and keeps prova's messages byte-for-byte: `send: session is closed`,
`terminal.spawn "cat": …`, `signal INT: kill(…) failed: …`.

Mocks and proxies stay in prova (ruling 2026-09-19). They are test-driver conveniences, and the
kernel should not carry harness concerns.

## The oracle

The kernel is measured against termlens (MIT OR Apache-2.0), used as a dev-dependency of its tests
and never shipped. `crates/prova-terminal/tests/oracle.rs` runs a corpus of fixture programs through
both. Each fixture names the aspects it targets: text rows, one cell's colour or attribute, cursor
position, visibility or shape, title, alternate screen, input modes, and a query's answer. An
adapter reads each side's screen, and on the kernel side it answers "cannot report" as a distinct
value. Wherever the two differ, the result is a key, `fixture/aspect`.

`tests/oracle.baseline` lists the admitted keys. It is the burn-down list, and its length is the
disagreement count, a lower-is-better **ratchet** that runs in `prova run ut`:
- **A new key fails.** It is a regression, or a new fixture whose gaps have not been admitted on
  purpose.
- **A listed key that stopped disagreeing also fails.** The list only shrinks, so a gain is locked
  in the moment it lands.

Both directions were seen red by name before the baseline counted (2026-09-19: 21 NEW against an
empty list, then one GONE against a padded one).

The seed count is 21. One of them is a DEFECT rather than a missing report: `Screen::line(n)` returns
vt100's *logical* line, so a soft-wrapped row reads as one long line while `cell(r, c)` addresses the
grid (`wrap/text`). The fixtures are deterministic by construction: each draws, then stays alive
until the host has looked; a screen is read after 250 ms of quiet, never at a child exit; and the
responder fixture turns echo off before it asks. The oracle passed three consecutive runs.

## Invariants and how each is enforced

| Invariant | Enforced by |
|---|---|
| The kernel has no Lua, no async runtime and no git dependency, so it is publishable | construction: its `Cargo.toml` names portable-pty, vt100 and libc only |
| A screen snapshot never races the reader | construction: `screen()` copies plain data out under the lock |
| The reader's end REASON survives (clean EOF vs failed read) | test: `a_real_pty_round_trip` asserts `Ended{why}` is non-empty |
| prova's surface and messages are unchanged | proof: `proofs/spec/terminal`, plus the face's `spawn_drives_a_real_pty_round_trip` |
| `stop` is idempotent and closes the session | test: `stop_closes_the_session` |
| Every disagreement with termlens is admitted by name, and the list never grows or goes stale | test: `the_kernel_disagrees_with_termlens_only_where_the_baseline_admits` (the ratchet) |

## Next slices (Substrate plan docs/plans/PTY_FIRST_CLASS.md)

1. ~~Extract the kernel with no behaviour change.~~
2. ~~**The oracle and its ratchet.**~~ Landed: see [The oracle](#the-oracle), seeded at 21.
3. **Burn down in the order the UI arc needs:**
   - ~~`Screen::line` addresses grid rows (the `wrap/text` defect); surface what vt100 0.15 already
     tracks~~ — slice 3a, oracle 21 → 7: grid `lines`, cursor position and visibility,
     italic/underline/reverse, title, alternate screen, application cursor, bracketed paste, mouse
     reporting. prova's Lua `screen:line(n)` changed with it (a fix, proven in proofs/spec/terminal
     and seen red against the old binary); the other fields have no Lua surface yet.
   - ~~Cursor shape, the query responder, `dim`~~ — slice 3b, oracle 7 → 3: vt100 0.16's
     `Callbacks` (DECSCUSR, the replies, the window title, which 0.16 no longer keeps on its
     `Screen`). The four other responder fixtures (DA2, DSR, CPR, text-area size) agreed with
     termlens on first contact. prova's Lua driver answers probes now — proven in
     proofs/spec/terminal and seen red against the old binary.
   - The remaining 3: blink, conceal and strikethrough, which vt100 0.16 parses and drops. They
     need a per-cell SGR tracker of our own.
   - event-driven waits (a notify per output chunk, not a 15 ms poll);
   - cursor shape (DECSCUSR) and visibility;
   - the query responder;
   - recovered styles;
   - mode-aware keys, paste and mouse;
   - `Screen` diff and a snapshot format.
4. **Substrate's face**, over a git-rev pin of this crate. Open question for that slice: `Session`
   owns its child, and Substrate's supervisor must own every process it runs. Either the face hands
   the kernel a child the supervisor spawned, or the kernel grows a transport-only constructor.
