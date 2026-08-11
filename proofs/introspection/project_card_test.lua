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
  t:expect(r.stdout):contains("whole-bar merge")
  t:expect(r.stdout):contains("pre-push sweep")
end)

prova.test("CONTEXT.md rides the card — beside the manifest, or tucked in a flat layout's .prova/", {
  covers = "docs/design/mcp-mode.md#project-card-self-teaching",
  proves = "house rules the generic topics cannot know (register, conventions) reach the agent through the card; the flat-manifest + state-nook shape — prova's own — used to have NO working tuck-away spot, so its context silently never rendered",
}, function(t)
  -- Sibling of the manifest: the documented spot, at whatever directory the manifest lives in.
  local beside = t:tempdir() .. "/beside"
  fs.mkdir(beside .. "/proofs")
  fs.write(beside .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(beside .. "/CONTEXT.md", "House rule: items end with Recorded YYYY-MM-DD.")
  local r = shell.run(prova.bin .. " learn project", { cwd = beside, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("Project context (`CONTEXT.md`)")
  t:expect(r.stdout):contains("Recorded YYYY-MM-DD")

  -- The tuck-away: a FLAT manifest at the root plus a `.prova/` state nook — the brief lives with
  -- prova's other files instead of becoming one more root file.
  local tucked = t:tempdir() .. "/tucked"
  fs.mkdir(tucked .. "/proofs")
  fs.mkdir(tucked .. "/.prova")
  fs.write(tucked .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(tucked .. "/.prova/CONTEXT.md", "House rule: never cargo fmt repo-wide.")
  local n = shell.run(prova.bin .. " learn project", { cwd = tucked, merge_stderr = true })
  t:expect(n.code, n.stdout):equals(0)
  t:expect(n.stdout):contains("never cargo fmt repo-wide")

  -- Absent everywhere: the nudge names both spots truthfully (it used to promise `.prova/` while
  -- discovery only read beside the manifest).
  local bare = t:tempdir() .. "/bare"
  fs.mkdir(bare .. "/proofs")
  fs.write(bare .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  local none = shell.run(prova.bin .. " learn project", { cwd = bare, merge_stderr = true })
  t:expect(none.stdout):contains("drop a `CONTEXT.md` beside the manifest")
end)
