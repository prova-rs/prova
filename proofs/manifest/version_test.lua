--- What version prova claims to be, and why a proof can ask.
---
--- A `cargo install --git` build used to report exactly the version of the release it was cut
--- from. A suite authored against an unreleased 0.11.0 passed locally and died in CI on `attempt
--- to call a nil value (field 'writes')` — the released 0.11.0 lacked the API the local one had,
--- `--version` agreed with itself, and nothing in the environment could tell them apart.
---
--- Two things fix that: a non-release build stamps `+dev.<sha>`, and the version is readable from
--- Lua so a proof can assert what it is running on rather than what it hopes.

prova.test("prova.version is the version the binary reports",
  { proves = "version: one source of truth — a proof asserting compatibility must see exactly what --version prints, or it is asserting about something else" }, function(t)
  local reported = shell.run(prova.bin .. " --version", { check = true }).stdout:match("prova%s+(%S+)")

  t:expect(prova.version, "exposed to Lua"):never():is_nil()
  t:expect(prova.version, "matches --version"):equals(reported)
end)

prova.test("the version is a semver the requires gate can compare",
  { proves = "version: the +dev marker is BUILD METADATA, not a prerelease — semver ignores metadata when comparing but excludes prereleases from ranges that do not name one, so `-dev` would make every dev build fail every [requires] prova gate" }, function(t)
  -- The distinction is invisible by eye and decisive in behaviour, so it is pinned here.
  local v = prova.version
  t:expect(v, "looks like semver"):matches("^%d+%.%d+%.%d+")
  t:expect(v:find("-", 1, true), "carries no prerelease segment"):is_nil()

  -- Whatever this build is, a range that admits its release line must admit it.
  local major, minor = v:match("^(%d+)%.(%d+)")
  local pkg = fs.tempdir()
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/proofs/a_test.lua",
    'prova.test("ran", function(t) t:expect(1):equals(1) end)\n')
  fs.write(pkg .. "/prova.toml",
    ('[requires]\nprova = ">= %s.%s"\n\n[run]\nproofs = ["proofs"]\n'):format(major, minor))

  local r = shell.run(prova.bin, { cwd = pkg, merge_stderr = true })
  t:expect(r.code, "a dev build still satisfies its own release line"):equals(0)
  t:expect(r.stdout, "the suite actually ran"):contains("ran")
end)
