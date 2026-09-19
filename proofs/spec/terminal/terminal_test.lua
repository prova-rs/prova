--- The `terminal` kernel transport — PTY-backed driving of interactive programs, with a screen
--- model as the observation layer (docs/design/mocks-proxies-drivers.md). Decisions pinned:
---
---   * ONE kernel API, not two per-OS ones: only the ALLOCATION differs by platform (openpty
---     on Unix, ConPTY on Windows, both behind portable-pty). ConPTY emits the same VT
---     sequences openpty does, so the screen model — the observation layer — is byte-for-byte
---     OS-agnostic. `terminal` is the user-facing word; `pty` stays the internal module name.
---   * Session surface: `:send`, `:expect` (observe-until-match, timeout'd — same idea as
---     `wait_for`; never a sleep), `:wait_stable` (settle the frame), `:screen`, `:resize`
---     (a real SIGWINCH), `:signal`, `:wait`. Lifecycle via ctx:manage — child killed and pty
---     restored on scope exit, LIFO, for free.
---   * `Screen` type: `:text`, `:line(n)`, `:cell(r,c)` (char + fg/bg/attrs), `:contains`,
---     `:matches_snapshot` (golden frames: first run writes, later runs compare).
---   * `terminal.mock` — the narrow true mock: the SUT shells out to an interactive CLI and
---     you shadow it on PATH with a scripted responder built on the same kernel pty primitive.
---
--- Bodies use only POSIX-portable programs (cat, sh, stty, printf); tests that need them are
--- gated `requires = { "unix" }` — the ConPTY twins land with a Windows runner + `must_run`
--- (the capability system already covers this; see mocks-proxies-drivers.md).

-- ── the driver: spawn / send / expect ────────────────────────────────────────────────────────

prova.test("spawn + send + expect — the interactive round-trip, no sleeps",
  { requires = { "unix" }, proves = "tier-a/terminal: the interactive round-trip — expect observes, never sleeps" }, function(t)
  local term = terminal.spawn(t, { cmd = { "cat" }, cols = 80, rows = 24 })
  term:send("hello\r")
  term:expect("hello")                        -- pty echo; blocks until match, with a timeout
end)

prova.test("the screen model observes styled cells, not just bytes",
  { requires = { "unix" }, proves = "tier-a/terminal: the observation layer is a screen — styled cells, not bytes" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", [[printf '\033[31mRED\033[0m plain']] },
    cols = 80, rows = 24,
  })
  term:wait_stable()                          -- settle the frame; never sleep

  local s = term:screen()
  t:expect(s:contains("RED plain")):is_true()
  t:expect(s:line(0)):contains("RED")
  t:expect(s:cell(0, 0).fg):equals("red")     -- styled-cell assertion
  t:expect(s:cell(0, 4).fg):never():equals("red")   -- the reset took
end)

prova.test("line(n) is a grid row — a soft-wrapped line spans rows, as its cells do",
  { requires = { "unix" },
    proves = "tier-a/terminal: line(n) addresses the grid; until 2026-09-19 it read the logical line, so a wrapped row returned the whole line while cell(r, c) addressed the grid (the kernel oracle's wrap/text)" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", [[printf '%0100d' 0 | tr 0 x; sleep 5]] },
    cols = 80, rows = 24,
  })
  term:wait_stable()
  local s = term:screen()
  t:expect(s:line(0)):equals(string.rep("x", 80))   -- the first row holds 80 columns
  t:expect(s:line(1)):equals(string.rep("x", 20))   -- the wrap continues on the next row
  t:expect(s:cell(1, 19).char):equals("x")           -- where cell(r, c) agrees it is
  t:expect(s:contains(string.rep("x", 100))):is_true()   -- contains still sees the whole line
end)

prova.test("a capability probe is answered — the terminal replies to device-attribute queries",
  { requires = { "unix" },
    proves = "tier-a/terminal: the kernel's query responder answers DA1 as a VT220 with ANSI colour (termlens's reply, byte for byte), so a program that probes its terminal runs instead of waiting out its timeout" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "bash", "-c", [[stty -echo; printf '\033[c'; IFS= read -r -t 3 -d c reply; printf 'reply:%s' "${reply#?}"; sleep 5]] },
    cols = 80, rows = 24,
  })
  term:expect("reply:[?62;22", { timeout = "2s" })   -- answered well inside the program's 3 s wait
end)

prova.test("the screen reports the cursor — position, visibility, and the shape the program asked for",
  { requires = { "unix" },
    proves = "tier-a/terminal: screen.cursor carries the 0-based position, DECTCEM visibility and the DECSCUSR shape/blink — the block-vs-bar cursor a modal editor switches, invisible in the grid" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", [[printf '\033[5;10Hx\033[6 q\033[?25l'; sleep 5]] },
    cols = 80, rows = 24,
  })
  term:wait_stable()
  local c = term:screen().cursor
  t:expect(c.row):equals(4)
  t:expect(c.col):equals(10)              -- after the x written at column 9
  t:expect(c.visible):equals(false)
  t:expect(c.shape):equals("bar")         -- DECSCUSR 6: a steady bar
  t:expect(c.blink):equals(false)

  local plain = terminal.spawn(t, { cmd = { "sh", "-c", [[printf 'p'; sleep 5]] }, cols = 80, rows = 24 })
  plain:wait_stable()
  t:expect(plain:screen().cursor.shape):equals("default")   -- never asked is not "block"
  t:expect(plain:screen().cursor.blink):is_nil()
end)

prova.test("out-of-band state is observable — the title, the alternate screen, the input modes",
  { requires = { "unix" },
    proves = "tier-a/terminal: screen.title / .alternate_screen / .modes report what a program switched on that the grid never shows" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", [[printf '\033]0;my-title\007\033[?1049h\033[?2004h\033[?1h\033[?1000halt'; sleep 5]] },
    cols = 80, rows = 24,
  })
  term:wait_stable()
  local s = term:screen()
  t:expect(s.title):equals("my-title")
  t:expect(s.alternate_screen):equals(true)
  t:expect(s.modes.bracketed_paste):equals(true)
  t:expect(s.modes.application_cursor):equals(true)
  t:expect(s.modes.mouse_reporting):equals(true)
end)

prova.test("every SGR attribute reaches the cell — a masked field is told apart from one printed in clear",
  { requires = { "unix" },
    proves = "tier-a/terminal: dim/italic/underline/reverse/blink/conceal/strikethrough reach screen:cell; a concealed cell keeps its char, and conceal says it is hidden" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", [[printf '\033[2mD\033[0m\033[3mI\033[0m\033[4mU\033[0m\033[7mR\033[0m\033[5mK\033[0m\033[9mS\033[0m\033[8mhunter2\033[0m clear'; sleep 5]] },
    cols = 80, rows = 24,
  })
  term:wait_stable()
  local s = term:screen()
  t:expect(s:cell(0, 0).dim):equals(true)
  t:expect(s:cell(0, 1).italic):equals(true)
  t:expect(s:cell(0, 2).underline):equals(true)
  t:expect(s:cell(0, 3).reverse):equals(true)
  t:expect(s:cell(0, 4).blink):equals(true)
  t:expect(s:cell(0, 5).strikethrough):equals(true)
  t:expect(s:cell(0, 6).char):equals("h")          -- the text is there, as a terminal holds it…
  t:expect(s:cell(0, 6).conceal):equals(true)      -- …and it is hidden
  t:expect(s:cell(0, 14).conceal):equals(false)    -- the reset took: "clear" is in clear
end)

prova.test("resize is a real SIGWINCH — the program observes the new geometry",
  { requires = { "unix" }, proves = "tier-a/terminal: resize is a real SIGWINCH the program observes" }, function(t)
  local term = terminal.spawn(t, { cmd = { "sh" }, cols = 80, rows = 24 })
  term:send("stty size\r")
  term:expect("24 80")

  term:resize(120, 40)
  term:send("stty size\r")
  term:expect("40 120")
end)

prova.test("signal delivery — prove clean Ctrl-C handling, not just teardown",
  { requires = { "unix" }, proves = "tier-a/terminal: signals prove clean Ctrl-C handling, not just teardown" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", 'trap "echo CAUGHT" INT; echo READY; while :; do sleep 1; done' },
    cols = 80, rows = 24,
  })
  term:expect("READY")            -- the trap is registered — signaling earlier would be a race
  term:signal("INT")
  term:expect("CAUGHT")
end)

prova.test("wait reaps the child and reports its exit code",
  { requires = { "unix" }, proves = "tier-a/terminal: wait reaps and reports the exit code" }, function(t)
  local term = terminal.spawn(t, { cmd = { "sh", "-c", "exit 3" }, cols = 80, rows = 24 })
  t:expect(term:wait().code):equals(3)
end)

prova.test("golden frames — a screen matches its committed snapshot",
  { requires = { "unix" },
    proves = "tier-a/terminal: golden frames ride the standard snapshot flow" }, function(t)
  local term = terminal.spawn(t, {
    cmd = { "sh", "-c", [[printf 'STABLE FRAME']] },
    cols = 80, rows = 24,
  })
  term:wait_stable()
  -- First run writes proofs/spec/terminal/__snapshots__/stable-frame, later runs compare;
  -- a mismatch renders a frame diff. The store convention is the kernel's, shared with every
  -- transport that snapshots.
  t:expect(term:screen()):matches_snapshot("stable-frame")
end)

-- ── the mock: shadow an interactive CLI on PATH ──────────────────────────────────────────────

prova.test("terminal.mock shadows a CLI on PATH with a scripted responder",
  { requires = { "unix" }, proves = "tier-a/terminal: the PATH-shadow mock scripts the other side of an interactive CLI" }, function(t)
  local fake = terminal.mock(t, { as = "greeter" })
  fake:expect("hello"):send("world\n")        -- the script: consume SUT output, answer it

  -- The SUT side: anything spawned with the mock's env resolves `greeter` to the shim.
  local r = shell.run("printf 'hello' | greeter", { env = fake.env })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("world")
end)
