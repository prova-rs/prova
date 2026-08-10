--- `prova capabilities` — what in my world that VARIES is available to me? The variable host
--- probes (docker / github / OS), then what THIS package's manifest and companion reference,
--- probed the same way. A fact that cannot be false on any machine is not a capability check:
--- the compiled-in batteries appear only when a slim build lacks one, and the unprobed
--- assumptions (network/internet) do not appear at all. A report (exit 0), never a gate — the
--- gate is `must_run` at run time. Host-agnostic assertions only.

prova.test("the report is the VARIABLE world: probes yes, batteries and assumptions no",
  function(t)
  local r = shell.run(prova.bin .. " capabilities", { merge_stderr = true })
  t:expect(r.code, "a report exits 0 whatever the host lacks"):equals(0)
  t:expect(r.stdout, "the named host probes"):contains("docker")
  t:expect(r.stdout, "a status per capability"):contains("MET")
  -- Always-available is not a check: full builds show the batteries as one footnote, never rows.
  t:expect(r.stdout):contains("batteries, not checks")
  t:expect(r.stdout:match("%f[%a]MET%s+sqlite"), "a compiled-in module is never a MET row"):is_nil()
  -- Unprobed assumptions are not reported as facts.
  t:expect(r.stdout:match("%f[%a]MET%s+network"), "an assumed capability is not a row"):is_nil()
end)

prova.test("the report includes what THIS package references — must_run, topologies, registrations", {
  proves = "'do I have llvm-cov available to me?' is answered by the report because the manifest names it — the package's variable world beside the host's",
}, function(t)
  local root = t:tempdir()
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", [=[
[run]
proofs = ["proofs"]

[profiles.cov]
must_run = ["sh", "definitely-not-a-tool-xyz"]
]=])
  fs.write(proj .. "/prova.lua", 'runtime.capability("blessed", function() return true end)\n')
  fs.write(proj .. "/proofs/one_test.lua", 'prova.test("g", function(t) t:expect(1):equals(1) end)\n')
  local r = shell.run(prova.bin .. " capabilities", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "unmet guarantees REPORT here; they gate only at run time"):equals(0)
  t:expect(r.stdout):contains("what this package references")
  t:expect(r.stdout:match("%f[%a]MET%s+sh"), "a PATH tool the manifest names, probed"):never():is_nil()
  t:expect(r.stdout):contains("must_run: profile `cov`")
  t:expect(r.stdout:match("UNMET%s+definitely%-not%-a%-tool%-xyz"), "the missing tool is the point of the report"):never():is_nil()
  t:expect(r.stdout):contains("blessed")
  t:expect(r.stdout):contains("registered in the companion")
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

prova.test("the binary teaches capabilities as ONE meaning — the registry uses `keywords`", {
  proves = "the former naming hazard (a registry `capabilities` field vs. the host vocabulary) was \
removed by renaming the registry field to `keywords`; the topic teaches the single meaning and the \
`keywords` split so an agent never conflates 'find a package' with 'probe this host'",
}, function(t)
  local catalog = shell.run(prova.bin .. " learn", { merge_stderr = true })
  t:expect(catalog.stdout, "the catalog names the topic"):contains("capabilities")
  local topic = shell.run(prova.bin .. " learn capabilities", { merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the two directions"):contains("must_run")
  t:expect(topic.stdout, "discovery is keywords, not capabilities"):contains("keywords")
end)
