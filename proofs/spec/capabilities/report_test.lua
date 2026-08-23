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

prova.test("the report includes what THIS package references — must_run, topologies, declarations", {
  covers = "docs/design/capabilities.md#capabilities-declared-in-the-manifest",
  proves = "'do I have llvm-cov available to me?' is answered by the report because the manifest names it — the package's variable world beside the host's",
}, function(t)
  local root = t:tempdir()
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", [=[
[run]
proofs = ["proofs"]

[capabilities]
blessed = { command = "sh", version = false }

[profiles.cov]
must_run = ["sh", "definitely-not-a-tool-xyz"]
]=])
  fs.write(proj .. "/proofs/one_test.lua", 'prova.test("g", function(t) t:expect(1):equals(1) end)\n')
  local r = shell.run(prova.bin .. " capabilities", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "unmet guarantees REPORT here; they gate only at run time"):equals(0)
  t:expect(r.stdout):contains("what this package references")
  t:expect(r.stdout:match("%f[%a]MET%s+sh"), "a PATH tool the manifest names, probed"):never():is_nil()
  t:expect(r.stdout):contains("must_run: profile `cov`")
  t:expect(r.stdout:match("UNMET%s+definitely%-not%-a%-tool%-xyz"), "the missing tool is the point of the report"):never():is_nil()
  -- A DECLARED capability appears with the kind of factory behind it: the report has to answer
  -- "what does this name mean here?", not only "does it hold?".
  t:expect(r.stdout):contains("blessed")
  t:expect(r.stdout):contains("command probe")
end)

prova.test("the report marks a declaration that OVERRIDES a built-in", {
  covers = "docs/design/capabilities.md#overriding-a-builtin-is-declared",
  proves = "overriding is safe only because it is visible: a reader of another repo assumes `docker` means prova's docker, so a redefinition must never print as an ordinary row",
  requires = { "unix" },
}, function(t)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[capabilities]\ndocker = { command = "sh", version = false }\n')
  fs.write(proj .. "/proofs/one_test.lua", 'prova.test("g", function(t) t:expect(1):equals(1) end)\n')
  local r = shell.run(prova.bin .. " capabilities", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("OVERRIDES the built-in")
end)

prova.test("`capabilities <name>` explains one capability: what ran, and what came back", {
  proves = "the diagnostic gap: an unmet capability reported only that it was unavailable, and a wrong-version skip only the numbers — never the command that produced them",
  requires = { "unix" },
}, function(t)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[capabilities]\n' ..
    'tool = { command = "sh", version = ["-c", "echo \'tool version 7.8.9\'"] }\n')
  fs.write(proj .. "/proofs/one_test.lua", 'prova.test("g", function(t) t:expect(1):equals(1) end)\n')
  local r = shell.run(prova.bin .. " capabilities tool", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "an explanation is a report, not a gate"):equals(0)
  t:expect(r.stdout, "the kind of declaration"):contains("command probe")
  t:expect(r.stdout, "the raw output of the version query"):contains("tool version 7.8.9")
  t:expect(r.stdout, "and the parsed version"):contains("7.8.9")
end)

prova.test("exactly one OS capability is met — unix XOR windows, whatever the host", function(t)
  local r = shell.run(prova.bin .. " capabilities", { merge_stderr = true })
  -- `%f[%a]` is a frontier: it matches MET only at a word boundary, so it never fires inside UNMET.
  local unix_met = r.stdout:match("%f[%a]MET%s+unix") ~= nil
  local windows_met = r.stdout:match("%f[%a]MET%s+windows") ~= nil
  t:expect(unix_met ~= windows_met, "this host is one OS; the other reads UNMET"):is_true()
end)

prova.test("capabilities takes at most one name", {
  proves = "a constraint must be quoted as ONE argument (`capabilities \"dotnet >= 9\"`); two positionals is a shell-quoting mistake worth naming rather than silently reading the first",
}, function(t)
  local r = shell.run({ prova.bin, "capabilities", "one", "two" }, { merge_stderr = true })
  t:expect(r.code):equals(2)
  t:expect(r.stdout .. r.stderr):contains("at most one")
  -- An unknown name is not an error: it is a legitimate question with the answer "nothing declares
  -- this, and it is not on PATH".
  local unknown = shell.run(prova.bin .. " capabilities frobnicate", { merge_stderr = true })
  t:expect(unknown.code, "explaining an undeclared name is a report, not a refusal"):equals(0)
  t:expect(unknown.stdout):contains("UNMET")
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
