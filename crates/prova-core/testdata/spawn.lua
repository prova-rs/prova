prova.test("spawn, observe running, stop", function(t)
  local proc = shell.spawn("sleep 30")
  t:expect(proc.pid):gt(0)
  t:expect(proc:running()):is_true()
  proc:stop()                       -- async: kill + reap (teardown machinery awaits it too)
  t:expect(proc:running()):is_false()
end)

prova.test("wait returns the exit code", function(t)
  local proc = shell.spawn("exit 3")
  t:expect(proc:wait()):equals(3)
end)

prova.test("stop is idempotent", function(t)
  local proc = shell.spawn("sleep 30")
  proc:stop()
  proc:stop()                       -- second stop is a no-op, not an error
  t:expect(proc:running()):is_false()
end)
