--- A spawn that never happened must say what it tried to spawn.
---
--- Every other failure `shell.run` can report interpolates the command — the wall timeout, the
--- idle kill — and `CommandSpec::display_name`'s own doc promises "the full command is still in
--- the error on failure". Spawn was the one path that broke that promise, and it is the path that
--- needs it most: the OS says "No such file or directory", which sends the reader looking for a
--- missing FILE when what is missing is the program.
---
--- Found three layers away from here, in a consumer package: a [capabilities] predicate shelled
--- out to a tool CI did not have, and the run died at manifest load with an ENOENT that named
--- neither the tool nor the predicate's command. The verdict was correct — a probe that cannot
--- answer must not wave the suite through — but the message made the cause a guess.
---
--- The subject is `prova.bin`: called directly in a proof body, `shell.run` exercises whichever
--- binary is CONDUCTING this suite, not the one under test.

--- Run `code` inside the subject and return whatever it wrote — the error text, captured.
local function in_subject(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true }).stdout
end

local MISSING = "prova-no-such-program-b7f3e1"

prova.test("a failed spawn names the program it could not start", {
  proves = "shell.run's spawn error carries the command, like every other error it raises",
}, function(t)
  local out = in_subject(([[
    local ok, err = pcall(function() shell.run({ %q, "--version" }) end)
    io.write(tostring(ok) .. "|" .. tostring(err))
  ]]):format(MISSING))

  t:expect(out, "the call raised rather than returning a result"):contains("false|")
  t:expect(out, "and the error names the program"):contains(MISSING)
  t:expect(out, "with its arguments, so the whole invocation is visible"):contains("--version")
end)

prova.test("a missing program is told it might not be on PATH", {
  proves = "ENOENT on spawn earns the one hint that is almost always the cause",
}, function(t)
  local out = in_subject(([[
    local ok, err = pcall(function() shell.run({ %q }) end)
    io.write(tostring(err))
  ]]):format(MISSING))

  t:expect(out, "the hint points at the real cause"):contains("on PATH")
  -- `check = false` forgives a non-zero EXIT; a process that never started has no exit to
  -- forgive, so it still raises. Pinned because that is exactly the assumption that produced
  -- the consumer bug — a probe wrote `check = false` and believed it was covered.
  local forgiving = in_subject(([[
    local ok, err = pcall(function() shell.run({ %q }, { check = false }) end)
    io.write(tostring(ok) .. "|" .. tostring(err))
  ]]):format(MISSING))
  t:expect(forgiving, "check = false does not turn a failed spawn into a result"):contains("false|")
  t:expect(forgiving, "and it is still named"):contains(MISSING)
end)

prova.test("the supervised path names it too", {
  proves = "a bound run spawns through the supervisor — a second site, the same promise",
}, function(t)
  -- Any of timeout / idle_timeout / first_byte routes through run_supervised, which spawns on
  -- its own path. The two had drifted apart before; this holds them together.
  local out = in_subject(([[
    local ok, err = pcall(function() shell.run({ %q }, { timeout = "10s" }) end)
    io.write(tostring(err))
  ]]):format(MISSING))
  t:expect(out, "the bounded path names the program as well"):contains(MISSING)
  t:expect(out, "and hints the same way"):contains("on PATH")
end)
