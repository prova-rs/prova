# prova-terminal

The terminal kernel under [prova](https://github.com/prova-rs/prova)'s `terminal` driver: run a
program in a real PTY and observe it through a VT screen model.

- `Session::spawn` allocates the PTY (openpty on unix, ConPTY on Windows, via `portable-pty`),
  starts the child and a reader thread that feeds a raw transcript and a `vt100` parser.
- `Session::send` / `resize` / `signal` / `try_wait` / `stop` drive it.
- `Session::screen` freezes a `Screen` (text, lines, cells with colours) under the reader's lock,
  so an assertion never races the program.
- Waits are **checks, not sleeps**: `check_expect` and `activity` answer immediately, and the host
  owns the loop and its bound. prova bounds waits by deadline; an agent host can bound them by
  progress. `diagnose` explains a stall: bytes read, the reader's end reason, the child's state
  and the process tree still alive under it.

The kernel owns the session and no policy: no Lua, no async runtime, no timeouts.
