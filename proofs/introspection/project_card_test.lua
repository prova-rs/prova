--- The project card (docs/design/mcp-mode.md#project-card-self-teaching): `prova learn project`
--- computes, at call time, everything an agent must "just know" to work in THIS package — where
--- prova's files live, where specs are written, where proofs go, which profiles exist and when to
--- use them, which switches are thrown by whom. The card replaces the CLAUDE.md prose a team
--- would otherwise hand-maintain; computed cards cannot drift.

local carded = prova.fixture("project-card-pkg", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", [=[
[run]
proofs = ["proofs"]

[profiles.ut]
description = "the unit-test leg, deputed via nextest"
proofs      = ["ut"]
switches    = ["ut"]
must_run    = ["cargo-nextest"]

[[specs.source]]
type = "directory"
path = "docs"
]=])
  fs.write(proj .. "/proofs/one_test.lua", [[
prova.test("green", function(t) t:expect(true):is_true() end)
]])
  return proj
end)

prova.test("the card names the profiles — description, selection, switches, guarantees", {
  covers = "docs/design/mcp-mode.md#project-card-self-teaching",
  proves = "'ut' alone does not convey 'these are the unit tests' — the author's description rides the listing, so which-profile-when is answered by the tool",
}, function(t)
  local proj = t:use(carded)
  local r = shell.run(prova.bin .. " learn project", { cwd = proj, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("prova run <name>")
  t:expect(r.stdout):contains("the unit-test leg, deputed via nextest")
  t:expect(r.stdout):contains("throws: ut")
  t:expect(r.stdout):contains("guarantees: cargo-nextest")
  -- The switches line: thrown-by-config, and the pointer at the live inventory.
  t:expect(r.stdout):contains("prova switches")
  -- Prova's own files: the manifest variant, the companion, the state dir.
  t:expect(r.stdout):contains(".prova/var/")
  t:expect(r.stdout):contains("prova capabilities")
end)

prova.test("the card names the spec sources, and says they are writable", {
  covers = "docs/design/manifest.md#spec-sources-are-queryable",
  proves = "'where may I write an obligation?' was answerable only by opening prova.toml — the guess-the-file failure the capture procedure names",
}, function(t)
  local proj = t:use(carded)
  local r = shell.run(prova.bin .. " learn project", { cwd = proj, merge_stderr = true })
  t:expect(r.stdout):contains("`docs` (directory, writable)")
  t:expect(r.stdout):contains("<!-- claim: id -->")
  -- A package that never opted in is told how to, not shown an empty section.
  local plain = t:tempdir() .. "/plain"
  fs.mkdir(plain .. "/proofs")
  fs.write(plain .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  local none = shell.run(prova.bin .. " learn project", { cwd = plain, merge_stderr = true })
  t:expect(none.stdout):contains("[[specs.source]]")
end)

prova.test("the quality interface is profiles, each named and described — the exemplar's own bar", {
  covers = "docs/design/verifiers.md#exclusive-quality-interface",
  proves = "the CLAUDE.md prose this replaced could drift from what CI runs; `prova run --list` is computed from the same manifest CI's legs select, so it cannot",
}, function(t)
  -- Self-referential on purpose: the claim is about THIS repo. prova.root is the repo root, and
  -- the manifest under test is the one CI's legs run through.
  local r = shell.run(prova.bin .. " run --list", { cwd = prova.root, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  for _, leg in ipairs({ "ut", "quality", "coverage", "all" }) do
    t:expect(r.stdout, "the `" .. leg .. "` leg exists"):contains(leg)
  end
  -- Descriptions ride the listing — "ut" alone does not convey "these are the unit tests".
  t:expect(r.stdout):contains("unit tests")
  t:expect(r.stdout):contains("ratcheted against the committed baseline")
  t:expect(r.stdout):contains("pre-push sweep")
end)
