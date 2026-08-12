-- Quality gate: token-level code duplication does not multiply. `jscpd` (min-tokens 100, so only
-- the egregious copy-paste registers — idiomatic Rust repetition stays under the bar) scans the
-- production source and the clone COUNT is ratcheted, exactly like the unwrap census. Semantic
-- DRY ("two paths that do the same thing") has no detector; token clones are the honest ceiling.
--
-- `jscpd` is a world fact (requires — the box without node skips visibly; CI installs it);
-- asking for the quality class is intent (the switch). The dirs scanned come from
-- `cargo metadata` (workspace.src_roots) — production source only, never target/, tests/, or
-- vendored deps. (The earlier `crates xtask` scan swept integration tests and Lua selftests in,
-- contradicting this comment's own "production source" claim; the baseline re-banked when the
-- scan narrowed to match the words.)

local workspace = require("workspace")

prova.test("token-level clone count does not regress past the baseline", {
  switch = "quality",
  -- `cargo metadata` READS workspace state (a build can rewrite Cargo.lock): coexists with
  -- other readers, waits out any build in any instance.
  locks = { prova.reads("cargo") },
  requires = { "jscpd", "cargo" },
}, function(t)
  local roots = workspace.src_roots(t:use(workspace.metadata))
  t:expect(#roots, "no source roots to scan — cargo metadata found no members"):gt(0)

  local out = prova.root .. "/target/jscpd"
  fs.remove_all(out)
  local cmd = { "jscpd", "--min-tokens", "100", "--reporters", "json", "--output", out,
    "--ignore", "**/testdata/**" }
  for _, root in ipairs(roots) do
    cmd[#cmd + 1] = root
  end
  shell.run(cmd, { cwd = prova.root, merge_stderr = true, timeout = "300s" })
  local report = json.decode(fs.read(out .. "/jscpd-report.json"))
  local clones = report.statistics.total.clones
  t:expect(clones, "the report carries a clone total"):gte(0)
  measure.ratchet(t, "rust.duplication.clones", clones, { set = "quality" })
end)
