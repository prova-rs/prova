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
