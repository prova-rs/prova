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
| `screen()` → a frozen `Screen` (`contents`, `line`, `contains`, `cell` → `Cell{ch, fg, bg, bold}`) | `Screen` userdata, `snapshot_text` for `matches_snapshot` |
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

## Invariants and how each is enforced

| Invariant | Enforced by |
|---|---|
| The kernel has no Lua, no async runtime and no git dependency, so it is publishable | construction: its `Cargo.toml` names portable-pty, vt100 and libc only |
| A screen snapshot never races the reader | construction: `screen()` copies plain data out under the lock |
| The reader's end REASON survives (clean EOF vs failed read) | test: `a_real_pty_round_trip` asserts `Ended{why}` is non-empty |
| prova's surface and messages are unchanged | proof: `proofs/spec/terminal`, plus the face's `spawn_drives_a_real_pty_round_trip` |
| `stop` is idempotent and closes the session | test: `stop_closes_the_session` |

## Next slices (Substrate plan docs/plans/PTY_FIRST_CLASS.md)

1. ~~Extract the kernel with no behaviour change.~~
2. **The oracle and its ratchet.** A dev-dependency runs the same fixture programs through termlens
   and through the kernel, diffs text, cursor, modes and styles, and counts the disagreements as a
   lower-is-better ratchet, seen red first.
3. **Burn down in the order the UI arc needs:**
   - event-driven waits (a notify per output chunk, not a 15 ms poll);
   - cursor shape (DECSCUSR) and visibility;
   - the query responder;
   - recovered styles;
   - mode-aware keys, paste and mouse;
   - `Screen` diff and a snapshot format.
4. **Substrate's face**, over a git-rev pin of this crate. Open question for that slice: `Session`
   owns its child, and Substrate's supervisor must own every process it runs. Either the face hands
   the kernel a child the supervisor spawned, or the kernel grows a transport-only constructor.
