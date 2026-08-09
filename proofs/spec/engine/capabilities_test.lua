--- `prova capabilities` — the host capability report. What can prova detect on THIS machine: the
--- built-in vocabulary (docker / github / OS / native clients), each MET or UNMET with a reason. A
--- report (exit 0), never a gate — the gate is `must_run` at run time. Host-agnostic assertions
--- only: which capabilities are present depends on the box, but the report's SHAPE does not.

prova.test("`prova capabilities` reports the built-in vocabulary with host status, and never gates",
  function(t)
  local r = shell.run(prova.bin .. " capabilities", { merge_stderr = true })
  t:expect(r.code, "a report exits 0 whatever the host lacks"):equals(0)
  t:expect(r.stdout, "the named host probes"):contains("docker")
  t:expect(r.stdout, "the native clients"):contains("sqlite")
  -- network/internet/the compiled natives are always available, so at least one line reads MET.
  t:expect(r.stdout, "a status per capability"):contains("MET")
end)

prova.test("exactly one OS capability is met — unix XOR windows, whatever the host", function(t)
  local r = shell.run(prova.bin .. " capabilities", { merge_stderr = true })
  -- `%f[%a]` is a frontier: it matches MET only at a word boundary, so it never fires inside UNMET.
  local unix_met = r.stdout:match("%f[%a]MET%s+unix") ~= nil
  local windows_met = r.stdout:match("%f[%a]MET%s+windows") ~= nil
  t:expect(unix_met ~= windows_met, "this host is one OS; the other reads UNMET"):is_true()
end)

prova.test("capabilities is a no-argument report", function(t)
  local r = shell.run(prova.bin .. " capabilities frobnicate", { merge_stderr = true })
  t:expect(r.code):equals(2)
  t:expect(r.stdout):contains("unexpected argument")
end)

prova.test("the binary teaches capabilities, and separates it from the registry's `capabilities`", {
  proves = "the naming hazard is real — a package's advertised `capabilities` tag (packages search) \
is catalog metadata, not a host probe; the topic must keep them apart or an agent conflates them",
}, function(t)
  local catalog = shell.run(prova.bin .. " learn", { merge_stderr = true })
  t:expect(catalog.stdout, "the catalog names the topic"):contains("capabilities")
  local topic = shell.run(prova.bin .. " learn capabilities", { merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the two directions"):contains("must_run")
  t:expect(topic.stdout, "disambiguated from registry metadata"):contains("prova packages")
end)
