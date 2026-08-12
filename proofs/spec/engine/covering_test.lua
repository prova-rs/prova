--- Claim-scoped selection (docs/design/agent-ergonomics.md#claim-scoped-selection): a definition
--- of done is a selection string. `--covering <claim>` selects exactly the proofs whose `covers`
--- discharge it, at three grains — full address, bare id, whole doc — composing with the other
--- axes and `--list`. The MCP twin is held equal by the selection-parity unit gates.

local function package(t)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(proj .. "/docs/design.md", [[
# design

<!-- claim: alpha-rule -->
The alpha invariant holds.

<!-- claim: beta-rule -->
The beta invariant holds.
]])
  fs.write(proj .. "/proofs/slice_test.lua", [[
prova.test("proves the alpha rule", { covers = "docs/design.md#alpha-rule" }, function(t)
  print("COVERING " .. json.encode(prova.selection.covering))
  t:expect(true):is_true()
end)
prova.test("proves the beta rule", { covers = "docs/design.md#beta-rule" }, function(t)
  t:expect(true):is_true()
end)
prova.test("covers nothing", function(t)
  t:expect(true):is_true()
end)
]])
  return proj
end

prova.test("--covering selects by full address, bare id, and whole doc", {
  covers = "docs/design/agent-ergonomics.md#claim-scoped-selection",
  proves = "the three grains a brief names a gate at: one claim's proof, one claim by its short name, and 'the acceptance for THIS spec' — each selecting exactly the discharging proofs, with everything else deselected rather than run-and-ignored",
}, function(t)
  local proj = package(t)

  local full = shell.run(prova.bin .. ' --covering "docs/design.md#alpha-rule"',
    { cwd = proj, merge_stderr = true })
  t:expect(full.code):equals(0)
  t:expect(full.stdout, "exactly the discharging proof"):contains("1 passed")
  t:expect(full.stdout, "the axis is a visible fact to the run itself")
    :contains('COVERING ["docs/design.md#alpha-rule"]')

  local bare = shell.run(prova.bin .. " --covering beta-rule", { cwd = proj, merge_stderr = true })
  t:expect(bare.code):equals(0)
  t:expect(bare.stdout):contains("1 passed")
  t:expect(bare.stdout, "the bare grain picked beta, not alpha"):never():contains("COVERING")

  local doc = shell.run(prova.bin .. " --covering docs/design.md", { cwd = proj, merge_stderr = true })
  t:expect(doc.code):equals(0)
  t:expect(doc.stdout, "the whole spec's acceptance"):contains("2 passed")
end)

prova.test("--covering composes with --list: the gate is enumerable before it runs", {
  covers = "docs/design/agent-ergonomics.md#claim-scoped-selection",
  proves = "an orchestrator wants to SHOW the gate in a brief, not just fire it — the selector must answer under --list exactly as it selects under a run",
}, function(t)
  local proj = package(t)
  local r = shell.run(prova.bin .. " --list --covering alpha-rule", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("proves the alpha rule")
  t:expect(r.stdout):never():contains("proves the beta rule")
  t:expect(r.stdout):never():contains("covers nothing")
end)
