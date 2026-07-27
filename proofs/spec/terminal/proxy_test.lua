--- `terminal.proxy` — the interpose posture for the pty transport, the one missing cell in the
--- transport matrix (docs/design/mocks-proxies-drivers.md). Completes the terminal triad
--- (terminal.mock terminates, terminal.spawn originates) AND unlocks the differentiated
--- record/replay story: a ConPTY session recorded on Windows once, replayed deterministically on
--- every platform (the cross-platform argument the whole terminal transport is built on).
---
--- Decisions these specs pin:
---
---   * `terminal.proxy(ctx, { as, upstream?, cassette?, mode? })` — shadows a command NAME on PATH
---     exactly as `terminal.mock` does (the SUT shells out to an interactive CLI), but instead of a
---     scripted responder it forwards to the REAL program on a pty and records the session, or
---     replays a recorded one. `.env` is the PATH-prefixed environment handed to whatever spawns
---     the SUT (terminal.spawn / shell.run).
---   * Modes are the standard four: passthrough | record | replay | auto. Record forwards + captures
---     (flushed on `:close()`); replay reproduces the recorded output with no real program.
---   * A terminal cassette is the FULL-DUPLEX kind (asciinema-shaped: ordered output frames, timing
---     annotated), the scripted-conversation model — NOT the VCR request/response discipline. Same
---     record-once-replay-forever ethos, different replay shape.
---
--- POSIX-portable bodies (sh/printf); the ConPTY twin lands with a Windows runner + must_run.

local function fake_cli(t, body)
  local cli = t:tempdir() .. "/realcli"
  fs.write(cli, "#!/bin/sh\n" .. body)
  shell.run({ "chmod", "+x", cli }, { check = true })
  return cli
end

prova.test("record forwards to the real CLI on a pty and flushes a cassette on close",
  { requires = { "unix" }, proves = "closing/terminal.proxy: record forwards to the real CLI on a pty; close flushes the cassette" }, function(t)
  local cli = fake_cli(t, "printf 'WELCOME v1\\n'\n")
  local cas = t:tempdir() .. "/session.cast"

  local rec = terminal.proxy(t, { as = "tool", upstream = cli, cassette = cas, mode = "record" })
  local term = terminal.spawn(t, { cmd = { "sh", "-c", "tool" }, env = rec.env })
  term:expect("WELCOME v1")            -- the REAL cli answered, through the proxy
  rec:close()                          -- the flush point

  t:expect(fs.exists(cas)):is_true()
end)

prova.test("replay reproduces the recorded session — the real CLI never runs",
  { requires = { "unix" }, proves = "closing/terminal.proxy: replay reproduces the session — the real CLI never runs" }, function(t)
  local cli = fake_cli(t, "printf 'WELCOME v1\\n'\n")
  local cas = t:tempdir() .. "/session.cast"

  local rec = terminal.proxy(t, { as = "tool", upstream = cli, cassette = cas, mode = "record" })
  terminal.spawn(t, { cmd = { "sh", "-c", "tool" }, env = rec.env }):expect("WELCOME v1")
  rec:close()

  -- Delete the real CLI: replay must not touch it — the cassette is the whole story.
  shell.run({ "rm", cli }, { check = true })

  local rep = terminal.proxy(t, { as = "tool", cassette = cas, mode = "replay" })
  local term = terminal.spawn(t, { cmd = { "sh", "-c", "tool" }, env = rep.env })
  term:expect("WELCOME v1")            -- reproduced from the cassette, deterministically
end)

prova.test("a terminal cassette recorded here replays here — the cross-platform mechanism",
  { requires = { "unix" }, proves = "closing/terminal.proxy: a portable cassette replays VT sequences intact — the cross-platform mechanism" },
  function(t)
  -- The unix half of the ConPTY story: record produces a portable cassette a DIFFERENT platform
  -- replays. Here both are unix; the Windows twin is a windows-gated `requires` on a runner.
  -- Green "OK" then plain "done": the raw stream carries VT sequences, the SCREEN renders text.
  local cli = fake_cli(t, "printf '\\033[32mOK\\033[0m done\\n'\n")
  local cas = t:tempdir() .. "/styled.cast"

  local rec = terminal.proxy(t, { as = "styler", upstream = cli, cassette = cas, mode = "record" })
  terminal.spawn(t, { cmd = { "sh", "-c", "styler" }, env = rec.env }):expect("done")
  rec:close()

  local rep = terminal.proxy(t, { as = "styler", cassette = cas, mode = "replay" })
  local term = terminal.spawn(t, { cmd = { "sh", "-c", "styler" }, env = rep.env })
  term:expect("done")                                     -- block until observed, then read the frame
  local s = term:screen()
  t:expect(s:contains("OK done")):is_true()               -- VT sequences replayed and rendered
  t:expect(s:cell(0, 0).fg):equals("green")               -- the color survived record→replay
end)
