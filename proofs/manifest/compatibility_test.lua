--- The manifest compatibility contract — how prova.toml survives prova getting better.
---
--- Written as one file because these are one mechanism: picking them off individually produces a
--- version gate nobody can read, or strictness with no escape hatch. Authored as promises ahead of
--- the implementation; the graduated ones carry `proves`, and `prova promises` names what is left.
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
    fs.mkdir(dir .. "/proofs")
    fs.write(dir .. "/proofs/a_test.lua",
      'prova.test("the sandbox proof runs", function(t) t:expect(1):equals(1) end)\n')
    fs.write(dir .. "/prova.toml", manifest)
    return dir
  end
end)

-- ── the version gate ─────────────────────────────────────────────────────────────────────────

prova.test("a manifest requiring a newer prova is refused up front",
  { proves = "compatibility: [requires] prova is a floor — it turns a mid-run `nil value` crash on an out-of-date binary into a precondition naming both versions" }, function(t)
  -- The failure this prevents is the one that actually happened: a suite authored against an
  -- unreleased binary crashed mid-run in CI with `attempt to call a nil value (field 'writes')`,
  -- which says nothing about the real cause. A precondition naming both versions does.
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, { cwd = pkg('[requires]\nprova = "99.0.0"\n\n[run]\nproofs = ["proofs"]\n'), merge_stderr = true })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "names the required version"):contains("99.0.0")
  local in_hand = shell.run(prova.bin .. " --version").stdout:match("(%d+%.%d+%.%d+)")
  t:expect(r.stdout, "names the version in hand"):contains(in_hand)
  t:expect(r.stdout, "no proof was attempted"):never():contains("the sandbox proof runs")
end)

prova.test("`>=` is how a suite says this-version-or-newer",
  { proves = "compatibility: a suite that must survive upgrades writes an open range — a bare version is a caret, and on 0.x a caret walls at the next minor" }, function(t)
  -- The half that keeps a declaration from freezing the suite to one line of releases. Written
  -- `>=`, it keeps running as prova advances — which is the whole promise the gate exists to make
  -- safe to rely on.
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, { cwd = pkg('[requires]\nprova = ">= 0.4"\n\n[run]\nproofs = ["proofs"]\n'), merge_stderr = true })

  t:expect(r.code, "runs"):equals(0)
  t:expect(r.stdout, "the proof executed"):contains("the sandbox proof runs")
end)

prova.test("one file, one answer: a plugin's requires.prova means the same to both readers",
  { proves = "compatibility: a prova.toml is one file wearing whichever hats it declares, so the version gate cannot mean a range to a plugin consumer and a floor to the plugin's own suite" }, function(t)
  -- The proof that pins the two readers together. `prova init plugin` scaffolds precisely this
  -- overlap — a plugin carrying its own self-test suite — so a single requires.prova is read once
  -- by the package gate and once by the plugin resolver. They disagreed: an out-of-range caret
  -- passed the plugin's own CI and was rejected by every consumer of it. Same verdict now, and
  -- this fails if either reader drifts.
  local dir = t:use(package_with)('[package]\nname = "p"\nentry = "p.lua"\n\n[requires]\nprova = "^0.5"\n\n[run]\nproofs = ["proofs"]\n')
  fs.write(dir .. "/p.lua", 'return { hello = function() return "hi" end }\n')

  local consumer = t:use(package_with)('[run]\nproofs = ["proofs"]\n\n[dependencies]\np = "' .. dir .. '"\n')
  fs.write(consumer .. "/proofs/use_test.lua",
    'local p = require("p")\nprova.test("uses it", function(t) t:expect(p.hello()):equals("hi") end)\n')

  local as_package = shell.run(prova.bin, { cwd = dir, merge_stderr = true })
  local as_plugin = shell.run(prova.bin, { cwd = consumer, merge_stderr = true })

  -- ^0.5 excludes every prova that will ever run this, so BOTH must refuse it.
  t:expect(as_package.code, "the package gate refuses it"):never():equals(0)
  t:expect(as_plugin.code, "the plugin resolver refuses it"):never():equals(0)
  t:expect(as_package.stdout, "the package gate names the range"):contains("^0.5")
  t:expect(as_plugin.stdout, "the plugin resolver names the range"):contains("^0.5")
end)

prova.test("the version gate is readable even when the rest of the manifest is not",
  { proves = "compatibility: the gate is read from generic TOML before the schema is applied, so a manifest written for a newer prova is diagnosed as out-of-date rather than as an unknown key" }, function(t)
  -- The bootstrap problem. A binary too old to understand a manifest must still be able to say
  -- so. That means phase one parses generic TOML and reads ONLY requires.prova; strict schema
  -- validation happens afterwards, once the version is known to be acceptable. Get this
  -- backwards and the gate reports "unknown key" precisely when it should report "too old".
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, {
    cwd = pkg('[requires]\nprova = "99.0.0"\n\n[run]\nproofs = ["proofs"]\nkey_from_the_future = true\n'),
    merge_stderr = true,
  })

  t:expect(r.stdout, "reports the version, not the unknown key"):contains("99.0.0")
  t:expect(r.stdout, "does not lead with the schema complaint"):never():contains("key_from_the_future")
end)

-- ── strictness, and the escape hatch that makes it affordable ────────────────────────────────

prova.test("an unknown key inside [run] is an error",
  { proves = "compatibility: [run] is a closed struct — ignoring a key here buys no forward compatibility worth having and costs a silently-dropped setting" }, function(t)
  -- [run] is a closed struct, not an open map. Ignoring a key here buys no forward
  -- compatibility worth having and costs a silent typo — the exact trade prova refuses
  -- everywhere else.
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, { cwd = pkg('[run]\nproofs = ["proofs"]\nbogus_key = 42\n'), merge_stderr = true })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "names the offending key"):contains("bogus_key")
end)

prova.test("[run.env] still accepts anything",
  { proves = "compatibility: strictness must not reach into genuinely open maps — environment names are user data, not schema" }, function(t)
  -- The counterweight. Environment names are user data, not schema; strictness that reached in
  -- here would be a regression dressed up as rigor.
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, {
    cwd = pkg('[run]\nproofs = ["proofs"]\n\n[run.env]\nANYTHING_AT_ALL = "fine"\n'),
    merge_stderr = true,
  })

  t:expect(r.code, "runs"):equals(0)
end)

prova.test("a near-miss key is reported as the typo it is",
  { proves = "compatibility: proximity to a known key is proof of a typo regardless of any version declaration, so this needs no version context — it is the cheapest error message in the system" }, function(t)
  -- No future version will add a key one edit away from an existing one, so proximity is proof
  -- of typo regardless of what the manifest declares. This is the cheapest error message in the
  -- system and it turns the worst failure — a key that quietly does nothing — into the best.
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, { cwd = pkg('[run]\nporofs = ["proofs"]\n'), merge_stderr = true })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "suggests the intended key"):matches("did you mean.*proofs")
end)

-- ── removals leave tombstones ────────────────────────────────────────────────────────────────

prova.test("the retired `paths` key reports its replacement",
  { proves = "compatibility: a removal is a tombstone, not a deletion — the retired key keeps an entry naming its replacement, so stale manifests self-correct instead of getting a bare unknown-field error" }, function(t)
  -- `paths` is being dropped pre-release, so nothing is owed to it — which makes it the right
  -- key to establish the pattern on, while the cost is zero. A generic "unknown key" would send
  -- every stale manifest, example and blog post to a search engine; a tombstone answers in
  -- place. Tombstones age out a major or two later, once did-you-mean can carry the stragglers.
  local pkg = t:use(package_with)
  local r = shell.run(prova.bin, { cwd = pkg('[run]\npaths = ["proofs"]\n'), merge_stderr = true })

  t:expect(r.code, "exits non-zero"):never():equals(0)
  t:expect(r.stdout, "names the dead key"):contains("paths")
  -- Not just "unknown field `paths`, expected one of `proofs`, ..." — serde's generic message
  -- already contains both words by accident, and satisfied a weaker version of this proof. A
  -- tombstone has to SAY it was retired and what replaced it.
  t:expect(r.stdout, "says the key was retired"):matches("removed[^\n]*0%.%d+")
  t:expect(r.stdout, "points at the replacement by name"):matches("use `?proofs`?")
end)

-- ── one vocabulary across every table ────────────────────────────────────────────────────────

prova.test("a suite's `paths` are literal paths, not directory-name patterns",
  { proves = "compatibility: `paths` and `proofs` name genuinely different things, which is why both survive — a suite addresses locations, [run] matches directory names at any depth" }, function(t)
  -- The reason the vocabulary is NOT unified. `[run] proofs = ["deep"]` finds `nested/deep`
  -- anywhere below the root, because it matches directory NAMES. A suite's `paths` resolve as
  -- written, so the same bare name is simply missing. Renaming one to the other would collapse
  -- two distinct concepts into one word and lose the distinction that makes each precise.
  local pkg = t:use(package_with)
  local dir = pkg('[run]\nproofs = ["proofs"]\n\n[suites.extra]\npaths = ["nested/deep"]\n')
  fs.mkdir(dir .. "/nested/deep")
  fs.write(dir .. "/nested/deep/b_test.lua",
    'prova.test("the declared suite runs", function(t) t:expect(1):equals(1) end)\n')

  local r = shell.run(prova.bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "the literal path resolves"):equals(0)
  t:expect(r.stdout, "the declared suite was discovered"):contains("the declared suite runs")
end)

prova.test("`proofs` inside a suite is rejected, not ignored",
  { proves = "compatibility: writing `proofs` in a suite is a category error that used to cost the entire declared suite, silently, with a green result" }, function(t)
  -- The actual defect, once the rename idea is off the table. Writing `proofs` here is a
  -- category error, and today it costs an entire declared suite: the key is dropped, the suite
  -- never runs, and the result is green. Strictness turns a vanished suite into a sentence.
  local pkg = t:use(package_with)
  local dir = pkg('[run]\nproofs = ["proofs"]\n\n[suites.extra]\nproofs = ["nested/deep"]\n')
  fs.mkdir(dir .. "/nested/deep")
  fs.write(dir .. "/nested/deep/b_test.lua",
    'prova.test("the declared suite runs", function(t) t:expect(1):equals(1) end)\n')

  local r = shell.run(prova.bin, { cwd = dir, merge_stderr = true })
  t:expect(r.code, "exits non-zero rather than dropping the suite"):never():equals(0)
  t:expect(r.stdout, "names the offending key"):contains("proofs")
end)
