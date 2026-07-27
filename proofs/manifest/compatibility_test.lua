--- The manifest compatibility contract — how prova.toml survives prova getting better.
---
--- Nothing here is implemented yet; every test is an open spec. They were written together
--- because they are one mechanism, and picking them off individually would produce a version
--- gate nobody can read, or strictness with no escape hatch.
---
--- The shape, decided before 1.0 while there is still nothing to honor:
---
---   floor, not wall  `[requires] prova` is a MINIMUM. Archetect walls off majors because its
---                    substrate changed three times (YAML → Rhai → Lua) and the artifacts are
---                    genuinely incompatible; prova has one substrate and intends newer
---                    binaries to keep running older suites. Copying the wall would make every
---                    0.x suite refuse the 1.0 binary on release day.
---   two-phase read   The gate must be readable by a binary that understands nothing else in
---                    the file, or it cannot do its job on exactly the manifests it exists for.
---   strict, scoped   Unknown keys inside a KNOWN table are typos and must fail. Unknown
---                    top-level tables stay lenient — that is where forward compatibility
---                    actually lives.
---   tombstones       A removal is not a deletion. A retired key keeps an entry that names its
---                    replacement, so every stale manifest and blog post self-corrects.
---
--- The through-line is prova's existing stance: never let "less ran than you asked for" look
--- like success. `must_run`, empty-selection-is-exit-2 and orphaned-proofs-is-exit-2 all say
--- it already. Silently ignoring a manifest key is the one place that stance is not applied.

--- Builds a throwaway package with a given manifest. Returns the package root; each call gets
--- its own directory, and all of them are torn down with the file scope.
local package_with = prova.fixture("manifest-sandbox", Scope.File, function(ctx)
  return function(manifest)
    local dir = ctx:tempdir()
    shell.run({ "mkdir", "-p", dir .. "/proofs" }, { check = true })
    fs.write(dir .. "/proofs/a_test.lua",
      'prova.test("the sandbox proof runs", function(t) t:expect(1):equals(1) end)\n')
    fs.write(dir .. "/prova.toml", manifest)
    return dir
  end
end)

-- ── the version gate ─────────────────────────────────────────────────────────────────────────

prova.test("a manifest requiring a newer prova is refused up front",
  { spec = "compatibility: the floor half of [requires] prova" }, function(t)
  -- The failure this prevents is the one that actually happened: a suite authored against an
  -- unreleased binary crashed mid-run in CI with `attempt to call a nil value (field 'writes')`,
  -- which says nothing about the real cause. A precondition naming both versions does.
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", { cwd = pkg('[requires]\nprova = "99.0.0"\n\n[run]\nproofs = ["proofs"]\n') })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "names the required version"):contains("99.0.0")
  t:expect(r.stdout, "names the version in hand"):contains("0.11")
  t:expect(r.stdout, "no proof was attempted"):never():contains("the sandbox proof runs")
end)

prova.test("a manifest requiring this prova or older runs normally",
  { proves = "compatibility: the floor is a floor, not a wall — a 0.4 suite must run on 0.11 and keep running on 1.x, so the comparison is ordering and never same-major" }, function(t)
  -- The half that keeps the gate from being a wall. A 0.4 suite must run on a 0.11 binary, and
  -- must keep running on 1.x — so the comparison is ordering, never "same major".
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", { cwd = pkg('[requires]\nprova = "0.4.0"\n\n[run]\nproofs = ["proofs"]\n') })

  t:expect(r.code, "runs"):equals(0)
  t:expect(r.stdout, "the proof executed"):contains("the sandbox proof runs")
end)

prova.test("the version gate is readable even when the rest of the manifest is not",
  { spec = "compatibility: two-phase read — the gate must survive an unreadable file" }, function(t)
  -- The bootstrap problem. A binary too old to understand a manifest must still be able to say
  -- so. That means phase one parses generic TOML and reads ONLY requires.prova; strict schema
  -- validation happens afterwards, once the version is known to be acceptable. Get this
  -- backwards and the gate reports "unknown key" precisely when it should report "too old".
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", {
    cwd = pkg('[requires]\nprova = "99.0.0"\n\n[run]\nproofs = ["proofs"]\nkey_from_the_future = true\n'),
  })

  t:expect(r.stdout, "reports the version, not the unknown key"):contains("99.0.0")
  t:expect(r.stdout, "does not lead with the schema complaint"):never():contains("key_from_the_future")
end)

-- ── strictness, and the escape hatch that makes it affordable ────────────────────────────────

prova.test("an unknown key inside [run] is an error",
  { spec = "compatibility: closed tables reject, open tables accept" }, function(t)
  -- [run] is a closed struct, not an open map. Ignoring a key here buys no forward
  -- compatibility worth having and costs a silent typo — the exact trade prova refuses
  -- everywhere else.
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", { cwd = pkg('[run]\nproofs = ["proofs"]\nbogus_key = 42\n') })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "names the offending key"):contains("bogus_key")
end)

prova.test("[run.env] still accepts anything",
  { proves = "compatibility: strictness must not reach into genuinely open maps — environment names are user data, not schema" }, function(t)
  -- The counterweight. Environment names are user data, not schema; strictness that reached in
  -- here would be a regression dressed up as rigor.
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", {
    cwd = pkg('[run]\nproofs = ["proofs"]\n\n[run.env]\nANYTHING_AT_ALL = "fine"\n'),
  })

  t:expect(r.code, "runs"):equals(0)
end)

prova.test("a near-miss key is reported as the typo it is",
  { spec = "compatibility: did-you-mean, independent of any version declaration" }, function(t)
  -- No future version will add a key one edit away from an existing one, so proximity is proof
  -- of typo regardless of what the manifest declares. This is the cheapest error message in the
  -- system and it turns the worst failure — a key that quietly does nothing — into the best.
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", { cwd = pkg('[run]\nporofs = ["proofs"]\n') })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "suggests the intended key"):matches("did you mean.*proofs")
end)

-- ── removals leave tombstones ────────────────────────────────────────────────────────────────

prova.test("the retired `paths` key reports its replacement",
  { spec = "compatibility: a removal is a tombstone, not a deletion" }, function(t)
  -- `paths` is being dropped pre-release, so nothing is owed to it — which makes it the right
  -- key to establish the pattern on, while the cost is zero. A generic "unknown key" would send
  -- every stale manifest, example and blog post to a search engine; a tombstone answers in
  -- place. Tombstones age out a major or two later, once did-you-mean can carry the stragglers.
  local pkg = t:use(package_with)
  local r = shell.run("prova 2>&1", { cwd = pkg('[run]\npaths = ["proofs"]\n') })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "names the dead key"):contains("paths")
  t:expect(r.stdout, "names the replacement"):contains("proofs")
end)

-- ── one vocabulary across every table ────────────────────────────────────────────────────────

prova.test("[suites.*] accepts `proofs`, like every other table",
  { spec = "compatibility: `paths` cannot die while one table still requires it" }, function(t)
  -- Today a suite takes `paths` and silently ignores `proofs` — so a project consolidating on
  -- the new spelling loses an entire declared suite with no warning and a green result. Same
  -- defect class as a skip reporting green, and the reason `paths` cannot be removed from
  -- [run] until this lands.
  local pkg = t:use(package_with)
  local dir = pkg('[run]\nproofs = ["proofs"]\n\n[suites.extra]\nproofs = ["more"]\n')
  shell.run({ "mkdir", "-p", dir .. "/more" }, { check = true })
  fs.write(dir .. "/more/b_test.lua",
    'prova.test("the declared suite runs", function(t) t:expect(1):equals(1) end)\n')

  local r = shell.run("prova 2>&1", { cwd = dir })

  t:expect(r.code, "runs"):equals(0)
  t:expect(r.stdout, "the declared suite was discovered"):contains("the declared suite runs")
end)
