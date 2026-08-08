--- Black-box surface of the generated-state directory: where prova's own run state lands, and the
--- one escape hatch that moves it.
---
--- The contract, in one line: **prova writes generated state only into a directory it owns, and that
--- directory ignores itself.** A package's tracked tree is never touched, so a failing run leaves
--- nothing to accidentally commit and no `.gitignore` entry to hand-maintain.
---
---   location   : all generated state lives under `<home>/.prova/var/` — the `--last-failed`
---                record and held-topology run-state alike. Nothing generated at the package root.
---   self-ignore: `var/.gitignore` of `*`, written on creation. It composes recursively — each
---                package ignores its OWN state and nobody else's, at any nesting depth.
---   lazy       : the dir materializes on the first state WRITE, never at startup. A package that
---                is only ever read (`--help`, `promises`, a plugin dir with no proofs) stays clean.
---   escape     : `PROVA_VAR_DIR` relocates state wholesale, for source trees prova cannot write
---                to (read-only checkouts, Nix/Bazel sandboxes). It is an escape hatch, NOT a
---                preference: outcomes are identical either way, a relative path is refused, and
---                the override announces itself so it can never be an invisible machine difference.

--- `Scope.Test`, deliberately: every proof here MUTATES the tree it runs against — creating state
--- directories, and in one case rewriting a proof to turn a run green. A shared fixture would leak
--- those mutations forward and make "nothing was written here" unassertable in any test but the first.
local sandbox = prova.fixture("state-dir-sandbox", Scope.Test, function(ctx)
  local root = ctx:tempdir()

  -- `pkg` — a package with one passing and one failing proof, so a run has something to record.
  fs.mkdir(root .. "/pkg/proofs")
  fs.write(root .. "/pkg/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/pkg/proofs/widget_test.lua", [[
prova.test("arithmetic holds", function(t)
  t:expect(1 + 1):equals(2)
end)

prova.test("the widget is finished", function(t)
  t:expect("not yet"):equals("finished")
end)
]])

  -- `nested` — a package holding a SECOND, independent package (its own prova.toml). The parent's
  -- run prunes it (`has_manifest`), so it is the lazy-creation and recursion case.
  fs.mkdir(root .. "/nested/proofs")
  fs.mkdir(root .. "/nested/inner/proofs")
  fs.write(root .. "/nested/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/nested/proofs/outer_test.lua", [[
prova.test("outer fails", function(t) t:expect(1):equals(2) end)
]])
  fs.write(root .. "/nested/inner/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/nested/inner/proofs/inner_test.lua", [[
prova.test("inner fails", function(t) t:expect(1):equals(2) end)
]])

  -- `other` — a second, unrelated package, for the shared-PROVA_VAR_DIR collision case.
  fs.mkdir(root .. "/other/proofs")
  fs.write(root .. "/other/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/other/proofs/other_test.lua", [[
prova.test("other fails", function(t) t:expect(1):equals(2) end)
]])

  return root
end)

--- Run `prova` in `dir`, optionally with an overridden state root. Failing runs are expected
--- throughout this file, so the exit code is never `check`ed. stderr is folded into stdout so the
--- assertions can see diagnostics; `run_raw` keeps the streams apart for the machine formats.
--- `shell.run`'s `env` EXTENDS the inherited environment, so an unset override is spelled as the
--- empty string — which prova treats as unset. That keeps every run here hermetic even when the
--- outer suite itself was invoked with PROVA_VAR_DIR set.
local function run(dir, args, var_dir)
  return shell.run(prova.bin .. " " .. (args or ""), {
    cwd = dir,
    env = { PROVA_VAR_DIR = var_dir or "" },
    merge_stderr = true,
  })
end

local function run_raw(dir, args, var_dir)
  return shell.run(prova.bin .. " " .. (args or ""), {
    cwd = dir,
    env = { PROVA_VAR_DIR = var_dir or "" },
  })
end

--- The `run_finished` tally out of a `--format json` stream (JSONL — one event object per line).
local function tally(out)
  for line in out:gmatch("[^\n]+") do
    local ok, doc = pcall(json.decode, line)
    if ok and type(doc) == "table" and doc.type == "run_finished" then
      return {
        passed = doc.passed,
        failed = doc.failed,
        skipped = doc.skipped,
        spec = doc.spec,
        deselected = doc.deselected,
      }
    end
  end
  return nil
end

-- ── location: state lands in prova's own directory, not the user's tree ──────────────────────

prova.test("a failing run records its state under .prova/var/, nothing at the package root",
  { covers = "docs/design/ide-and-layout.md#state-dir-self-owning" }, function(t)
  local pkg = t:use(sandbox) .. "/pkg"
  local r = run(pkg)
  t:expect(r.stdout):contains("finished")                        -- the run really did fail

  t:expect(pkg .. "/.prova/var/last-failed.json"):exists()
  t:expect(fs.exists(pkg .. "/.last-failed.json"), "no state at the package root"):equals(false)
end)

prova.test("the state directory ignores itself, so the tracked tree stays clean",
  { covers = "docs/design/ide-and-layout.md#state-dir-self-owning" }, function(t)
  local pkg = t:use(sandbox) .. "/pkg"
  run(pkg)
  t:expect(pkg .. "/.prova/var/.gitignore"):exists()
  t:expect(fs.read(pkg .. "/.prova/var/.gitignore")):contains("*")
end)

prova.test("--last-failed round-trips from the new location", function(t)
  local pkg = t:use(sandbox) .. "/pkg"
  run(pkg)
  local r = run(pkg, "--last-failed")
  -- Exactly the failed node re-runs; the passing one is deselected.
  t:expect(r.stdout):contains("finished")
  t:expect(r.stdout):never():contains("arithmetic holds")
end)

prova.test("a green run clears the record but leaves the directory in place", function(t)
  local root = t:use(sandbox)
  local pkg = root .. "/pkg"
  run(pkg)
  t:expect(pkg .. "/.prova/var/last-failed.json"):exists()

  -- Make the failing proof pass, then re-run: the record goes, the dir stays.
  fs.write(pkg .. "/proofs/widget_test.lua", [[
prova.test("arithmetic holds", function(t)
  t:expect(1 + 1):equals(2)
end)
]])
  run(pkg)
  t:expect(fs.exists(pkg .. "/.prova/var/last-failed.json"), "record cleared"):equals(false)
  t:expect(pkg .. "/.prova/var"):is_dir()
end)

-- ── lazy + recursive: one state dir per package actually RUN, ignoring itself at any depth ────

prova.test("a read-only invocation never creates a state directory",
  { covers = "docs/design/ide-and-layout.md#state-dir-self-owning" }, function(t)
  local pkg = t:use(sandbox) .. "/pkg"
  local r = run(pkg, "promises")
  t:expect(r.code):equals(0)
  t:expect(fs.exists(pkg .. "/.prova/var"), "nothing written by an enumeration"):equals(false)
end)

prova.test("a nested package's state is its own, and a parent run never creates it",
  { covers = "docs/design/ide-and-layout.md#state-self-ignore-composes" }, function(t)
  local root = t:use(sandbox)
  local outer, inner = root .. "/nested", root .. "/nested/inner"

  -- The parent run prunes the nested package, so it must not leave state inside it.
  run(outer)
  t:expect(outer .. "/.prova/var/last-failed.json"):exists()
  t:expect(fs.exists(inner .. "/.prova/var"), "parent run leaves the child alone"):equals(false)

  -- Run the child on its own: it gets its OWN state dir, self-ignoring, at its own depth.
  run(inner)
  t:expect(inner .. "/.prova/var/last-failed.json"):exists()
  t:expect(inner .. "/.prova/var/.gitignore"):exists()
end)

-- ── the escape hatch: relocation without divergence ──────────────────────────────────────────

prova.test("PROVA_VAR_DIR relocates state wholesale, leaving the package untouched",
  { covers = "docs/design/ide-and-layout.md#var-dir-escape-hatch-rules" }, function(t)
  local root = t:use(sandbox)
  local pkg, elsewhere = root .. "/pkg", root .. "/state-root"

  run(pkg, nil, elsewhere)
  t:expect(fs.exists(pkg .. "/.prova/var"), "nothing written into the package"):equals(false)
  -- Keyed by package inside the root, so the record is somewhere below it — not at a fixed name.
  t:expect(#fs.glob(elsewhere, "**/last-failed.json"), "the record moved"):equals(1)

  -- The relocated record is live state, not a write-only copy: --last-failed still selects from it.
  local r = run(pkg, "--last-failed", elsewhere)
  t:expect(r.stdout):contains("finished")
  t:expect(r.stdout):never():contains("arithmetic holds")
end)

prova.test("PROVA_VAR_DIR changes where state lives and nothing else — same outcomes either way",
  { covers = "docs/design/ide-and-layout.md#var-dir-escape-hatch-rules" }, function(t)
  local root = t:use(sandbox)
  local pkg = root .. "/pkg"

  -- The ethos, mechanically pinned: an escape hatch that alters results is a "works on my machine"
  -- generator. The reported tally must be identical with and without the override — and the
  -- announcement must live on stderr, so the machine-readable stream is unaffected too.
  local plain = run_raw(pkg, "--format json")
  local moved = run_raw(pkg, "--format json", root .. "/state-root-2")
  t:expect(moved.code, "same exit status"):equals(plain.code)
  t:expect(type(tally(plain.stdout)), "the plain run reported a tally"):equals("table")
  t:expect(tally(moved.stdout)):equals(tally(plain.stdout))
end)

prova.test("PROVA_VAR_DIR is a state ROOT: two packages sharing it never collide",
  { covers = "docs/design/ide-and-layout.md#var-dir-escape-hatch-rules" }, function(t)
  local root = t:use(sandbox)
  local shared = root .. "/shared-state"

  run(root .. "/pkg", nil, shared)
  run(root .. "/other", nil, shared)

  -- Two packages, two records. One flat file would mean the second run silently clobbered the
  -- first — the failure mode that makes a shared cache volume unusable across a monorepo.
  t:expect(#fs.glob(shared, "**/last-failed.json"), "one record per package"):equals(2)

  -- And each package still reads back its OWN failures.
  local r = run(root .. "/pkg", "--last-failed", shared)
  t:expect(r.stdout):contains("finished")
  t:expect(r.stdout):never():contains("other fails")
end)

prova.test("a relative PROVA_VAR_DIR is refused, not resolved against the cwd",
  { covers = "docs/design/ide-and-layout.md#var-dir-escape-hatch-rules" }, function(t)
  local pkg = t:use(sandbox) .. "/pkg"
  -- prova runs from anywhere inside a package (discovery walks up), so a relative override would
  -- put state in a different place depending on where you invoked it. That is exactly the
  -- inconsistency the escape hatch must not introduce, so it is an error rather than a warning.
  local r = run(pkg, nil, "some/relative/path")
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("PROVA_VAR_DIR")
  t:expect(r.stdout):contains("absolute")
end)

prova.test("an overridden state root announces itself, so it is never an invisible difference",
  { covers = "docs/design/ide-and-layout.md#var-dir-escape-hatch-rules" }, function(t)
  local root = t:use(sandbox)
  local plain = run(root .. "/pkg")
  local moved = run(root .. "/pkg", nil, root .. "/state-root-3")

  t:expect(moved.stdout):contains("PROVA_VAR_DIR")
  t:expect(plain.stdout):never():contains("PROVA_VAR_DIR")   -- silent when unset: no new noise
end)
