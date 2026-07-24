-- The two constructors are modes, not kinds, so either can re-mode what the other made.
--
-- `prova.reads(prova.port(8080))` widens a port — exclusive on its own — into a concurrent hold, so
-- these two overlap where `resource_exclusive.lua`'s pair (same token, writer mode) serializes. That
-- pair of files is the whole point: identical token, opposite scheduling, decided only by the mode.
--
-- And `prova.writes` on a bare token serializes, the same way a bare string does. `record` is
-- injected by the harness (tests/resources.rs).
prova.test("port as reader (a)", { resources = { prova.reads(prova.port(8080)) } }, function(t)
  record("enter ra")
  prova.sleep(40)
  record("exit ra")
  t:expect(1):equals(1)
end)

prova.test("port as reader (b)", { resources = { prova.reads(prova.port(8080)) } }, function(t)
  record("enter rb")
  prova.sleep(40)
  record("exit rb")
  t:expect(1):equals(1)
end)

prova.test("named writer (a)", { resources = { prova.writes("db") } }, function(t)
  record("enter wa")
  prova.sleep(40)
  record("exit wa")
  t:expect(1):equals(1)
end)

prova.test("named writer (b)", { resources = { prova.writes("db") } }, function(t)
  record("enter wb")
  prova.sleep(40)
  record("exit wb")
  t:expect(1):equals(1)
end)
