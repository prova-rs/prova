-- Quality gate: token-level code duplication does not multiply. `jscpd` (min-tokens 100, so only
-- the egregious copy-paste registers — idiomatic Rust repetition stays under the bar) scans the
-- production source and the clone COUNT is ratcheted, exactly like the unwrap census. Semantic
-- DRY ("two paths that do the same thing") has no detector; token clones are the honest ceiling.
--
-- `jscpd` is a world fact (requires — the box without node skips visibly; CI installs it);
-- asking for the quality class is intent (the switch).

prova.test("token-level clone count does not regress past the baseline", {
  switch = "quality",
  requires = { "jscpd" },
}, function(t)
  local out = prova.root .. "/target/jscpd"
  fs.remove_all(out)
  shell.run(
    { "jscpd", "--min-tokens", "100", "--reporters", "json", "--output", out,
      "--ignore", "**/testdata/**", "crates", "xtask" },
    { cwd = prova.root, merge_stderr = true, timeout = "300s" }
  )
  local report = json.decode(fs.read(out .. "/jscpd-report.json"))
  local clones = report.statistics.total.clones
  t:expect(clones, "the report carries a clone total"):gte(0)
  measure.ratchet(t, "rust.duplication.clones", clones, { set = "quality" })
end)
