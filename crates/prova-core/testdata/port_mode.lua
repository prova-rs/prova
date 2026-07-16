--- `prova.ports` exposes the verb's port mode to topology/plugin authors (`"auto"` for tests and
--- default `prova up`; `"fixed"` for `prova up --fixed`). The driving test sets EXPECTED_PORTS to the
--- mode it configured, so this asserts the RunConfig's PortMode reaches Lua intact.
prova.test("prova.ports matches the run's port mode", function(t)
  t:expect(prova.ports):equals(os.getenv("EXPECTED_PORTS"))
end)
