--- `prova specs backfill` — the reverse of `owed`. `owed` finds claims no proof covers (prose owed a
--- test); backfill finds proofs no claim backs (a test owed a spec). A red→green worklist that
--- GATES — exit non-zero while any proof is unbacked — so an agent can drive spec-coverage to
--- complete. It NEVER writes the spec: it names the proof and the agent infers the claim, because an
--- auto-stubbed `<!-- claim -->` would be vacuous prose — the exact "empty green" `falsify` exists to
--- catch, one lane over.

local function mkpkg(ctx, body)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/widget_test.lua", body)
  return proj
end

prova.test("backfill lists proofs no claim backs and gates; a proof with `covers` is not listed",
  function(t)
  local proj = mkpkg(t, [[
prova.test("the backed one", { covers = "docs/x.md#thing" }, function(t) t:expect(1):equals(1) end)
prova.test("the unbacked one", function(t) t:expect(1):equals(1) end)
]])
  local r = shell.run(prova.bin .. " specs backfill", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "an unbacked proof is a red condition"):equals(1)
  t:expect(r.stdout, "the unbacked proof is named"):contains("the unbacked one")
  t:expect(r.stdout, "a proof with `covers` is backed, not listed"):never():contains("the backed one")
end)

prova.test("backfill is COMPLETE when every proof is backed — exit 0, nothing owed", function(t)
  local proj = mkpkg(t, [[
prova.test("one", { covers = "docs/x.md#a" }, function(t) t:expect(1):equals(1) end)
prova.test("two", { covers = "docs/x.md#b" }, function(t) t:expect(1):equals(1) end)
]])
  local r = shell.run(prova.bin .. " specs backfill", { cwd = proj, merge_stderr = true })
  t:expect(r.code, "all backed ⇒ complete"):equals(0)
  t:expect(r.stdout):contains("every proof is backed")
end)

prova.test("backfill writes nothing — it is a read-only gate, never a spec generator", {
  proves = "auto-stubbing an anchor for every unbacked proof would manufacture vacuous prose; the \
worklist names the gap and the human writes the claim that means something",
}, function(t)
  local proj = mkpkg(t, [[
prova.test("bare", function(t) t:expect(1):equals(1) end)
]])
  local r = shell.run(prova.bin .. " specs backfill", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(1)
  -- A read-only discovery: no run artifacts, no IDE wiring, and above all no fabricated spec doc.
  t:expect(fs.exists(proj .. "/.luarc.json"), "backfill does not wire the IDE"):equals(false)
  t:expect(fs.exists(proj .. "/.prova/var"), "backfill records no run state"):equals(false)
  t:expect(fs.exists(proj .. "/docs"), "backfill fabricates no spec doc"):equals(false)
end)
