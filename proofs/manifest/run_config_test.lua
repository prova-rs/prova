--- Black-box surface of run configuration: how a profile overlays `[run]`, where a knob's value
--- comes from when several sources compete, and the refusal to let an empty resolution look green.
---
--- The contract (docs/design/manifest.md): a profile field replaces the base's when present,
--- except `env` (merged per key, profile wins) and `must_run` (union — a profile promises more,
--- never less). Precedence for any knob is uniformly CLI flag > env var > manifest > auto-detect.
--- And an empty resolution — no proofs, a profile that doesn't exist, a selection matching
--- nothing — is an error, never a silent green run.

--- Builds a throwaway package with a given manifest and one passing proof. Each call gets its
--- own directory; all of them are torn down with the file scope.
local package_with = prova.fixture("run-config-sandbox", Scope.File, function(ctx)
  local nth = 0
  return function(manifest)
    -- Named per call, so each sandbox is its own directory AND says so on disk.
    nth = nth + 1
    local dir = ctx:tempdir(tostring(nth))
    fs.mkdir(dir .. "/proofs")
    fs.write(dir .. "/proofs/a_test.lua",
      'prova.test("the sandbox proof runs", function(t) t:expect(1):equals(1) end)\n')
    fs.write(dir .. "/prova.toml", manifest)
    return dir
  end
end)

--- Run `prova` in `dir`. `env` EXTENDS the inherited environment (empty string = unset), which
--- keeps every run hermetic even when the outer suite was invoked with its own overrides.
local function run(dir, args, env)
  return shell.run(prova.bin .. (args and (" " .. args) or ""), {
    cwd = dir,
    env = env or {},
    merge_stderr = true,
  })
end

-- ── profile overlay: replace, except env (per-key merge) and must_run (union) ────────────────

prova.test("a profile field replaces the base's when present",
  { covers = "docs/design/manifest.md#profile-overlay-semantics" }, function(t)
  -- `junit` makes replacement observable on disk: one report appears, at the profile's path.
  local dir = t:use(package_with)([[
[run]
proofs = ["proofs"]
junit = "base.xml"

[profiles.ci]
junit = "profile.xml"
]])
  local r = run(dir, "--profile ci")
  t:expect(r.code):equals(0)
  t:expect(dir .. "/profile.xml"):exists()
  t:expect(fs.exists(dir .. "/base.xml"), "replaced, not written alongside"):equals(false)
end)

prova.test("profile env merges per key: the profile wins where both speak, base keys survive",
  { covers = "docs/design/manifest.md#profile-overlay-semantics" }, function(t)
  local dir = t:use(package_with)([[
[run]
proofs = ["proofs"]
[run.env]
BASE_ONLY = "base"
SHARED = "from-base"

[profiles.ci]
[profiles.ci.env]
SHARED = "from-profile"
]])
  fs.write(dir .. "/proofs/env_test.lua", [[
prova.test("reports its environment", function(t)
  print("BASE_ONLY=" .. tostring(os.getenv("BASE_ONLY")))
  print("SHARED=" .. tostring(os.getenv("SHARED")))
  t:expect(true):equals(true)
end)
]])
  local base = run(dir)
  t:expect(base.stdout):contains("SHARED=from-base")

  local prof = run(dir, "--profile ci")
  t:expect(prof.stdout, "the shared key is the profile's"):contains("SHARED=from-profile")
  t:expect(prof.stdout, "the base-only key survives the overlay"):contains("BASE_ONLY=base")
end)

prova.test("must_run unions: a base guarantee is still enforced under a profile",
  { covers = "docs/design/manifest.md#profile-overlay-semantics" }, function(t)
  -- The additive direction is the one worth pinning: a profile that says nothing about
  -- `must_run` must not quietly drop the package baseline — a profile promises MORE, never less.
  local dir = t:use(package_with)([[
[run]
proofs = ["proofs"]
must_run = ["no-such-capability-anywhere"]

[profiles.lax]
jobs = 2
]])
  local r = run(dir, "--profile lax")
  t:expect(r.code, "a broken environment is exit 2, never a skip"):equals(2)
  t:expect(r.stdout):contains("guarantees")
  t:expect(r.stdout, "names the unmet capability"):contains("no-such-capability-anywhere")
end)

-- ── knob precedence: CLI flag > env var > manifest ───────────────────────────────────────────

prova.test("one knob, three sources: the flag beats the env var beats the manifest",
  { covers = "docs/design/manifest.md#knob-precedence" }, function(t)
  -- `color` makes the winner visible in the bytes: shell.run captures a pipe, so any styling
  -- seen here is `always` doing the work, never terminal detection. NO_COLOR/CLICOLOR_FORCE are
  -- neutralized because they influence only `auto` — this test is about the explicit chain.
  local dir = t:use(package_with)('[run]\nproofs = ["proofs"]\ncolor = "always"\n')
  local quiet = { PROVA_COLOR = "", NO_COLOR = "", CLICOLOR_FORCE = "" }

  local manifest_only = run(dir, nil, quiet)
  t:expect(manifest_only.stdout, "the manifest's `always` styles a pipe"):contains("\27[")

  local env_wins = run(dir, nil, { PROVA_COLOR = "never", NO_COLOR = "", CLICOLOR_FORCE = "" })
  t:expect(env_wins.stdout, "the env var overrides the manifest"):never():contains("\27[")

  local flag_wins = run(dir, "--color always",
    { PROVA_COLOR = "never", NO_COLOR = "", CLICOLOR_FORCE = "" })
  t:expect(flag_wins.stdout, "the flag overrides the env var"):contains("\27[")
end)

-- ── an empty resolution is an error, not a silent green run ──────────────────────────────────

prova.test("a package that resolves to no proofs is exit 2, not zero-of-zero green",
  { covers = "docs/design/manifest.md#empty-resolution-is-an-error" }, function(t)
  local dir = t:use(package_with)('[run]\nproofs = ["proofs"]\n')
  fs.remove_all(dir .. "/proofs")

  local r = run(dir)
  t:expect(r.code):equals(2)
  t:expect(r.stdout, "says what it looked for"):contains("no declaration files found")
  t:expect(r.stdout, "teaches the preferred spelling"):contains(".prova.lua")
end)

prova.test("a --profile name that doesn't exist is refused by name",
  { covers = "docs/design/manifest.md#empty-resolution-is-an-error" }, function(t)
  local dir = t:use(package_with)('[run]\nproofs = ["proofs"]\n')
  local r = run(dir, "--profile nope")
  t:expect(r.code):equals(2)
  t:expect(r.stdout):contains("no such profile")
  t:expect(r.stdout, "names the missing profile"):contains("nope")
end)

prova.test("a selection that matches nothing is exit 2 — and --allow-empty is the opt-out",
  { covers = "docs/design/manifest.md#empty-resolution-is-an-error" }, function(t)
  local dir = t:use(package_with)('[run]\nproofs = ["proofs"]\n')

  local r = run(dir, "-k zzz_matches_nothing")
  t:expect(r.code, "usually a typo, so it fails"):equals(2)
  t:expect(r.stdout, "points at the opt-out"):contains("--allow-empty")

  local allowed = run(dir, "--allow-empty -k zzz_matches_nothing")
  t:expect(allowed.code, "selecting nothing on purpose is fine"):equals(0)
  t:expect(allowed.stdout, "still says what was deselected"):contains("deselected")
end)
