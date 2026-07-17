-- The argv form of `shell.run` / `shell.spawn`: no shell, no quoting — mirroring `container:run`,
-- whose docs sell exactly that. Its absence was the local half of the SDK lacking what the
-- containerized half had. See docs/design/agent-ergonomics.md §1.

prova.test("an argv table runs the program directly, preserving arguments verbatim", function(t)
  -- A shell would collapse the run of spaces; argv hands the argument over untouched.
  local r = shell.run({ "echo", "hello   world" })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):equals("hello   world\n")
end)

prova.test("argv passes content a shell string would EXECUTE, as data", function(t)
  -- The whole point: this is what you get when a test passes SQL/source/JSON to a local CLI.
  -- Through a shell string, `;` starts a command and `$(…)` substitutes. Through argv, neither
  -- can happen — there is no shell to interpret them.
  local nasty = [[a "b" ; echo INJECTED $(echo SUBSTITUTED)]]
  local r = shell.run({ "echo", nasty })
  t:expect(r.stdout):equals(nasty .. "\n")
  t:expect(r.stdout):never():contains("INJECTED\n")
end)

prova.test("a string command still goes through a shell", function(t)
  -- The string form is unchanged and still ergonomic for fixed commands.
  local r = shell.run("echo one && echo two")
  t:expect(r.stdout):equals("one\ntwo\n")
end)

prova.test("shell.spawn takes argv too", function(t)
  local proc = t:manage(shell.spawn({ "sleep", "30" }))
  t:expect(proc.pid):is_truthy()
end)

prova.test("a non-string, non-table command fails with an actionable message", function(t)
  -- Previously an argv attempt died as "bad argument #1: error converting Lua table to String",
  -- which names the conversion, not the fix.
  local ok, err = pcall(function() return shell.run(42) end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):contains("argv table")
end)
