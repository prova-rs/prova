--- Coverage, layered (docs/design/verifiers.md#coverage-of-the-whole-bar): ONE conduct, three
--- numbers. The unit layer (`cargo llvm-cov nextest`) and the black-box layer (the proof suite
--- through an instrumented prova, every `prova.bin` child writing its own profraw) are staged,
--- so each layer reports alone AND merged — the DELTA is the signal: a file rich in black-box
--- coverage but naked at the unit layer is proven behavior with no fast local feedback, the
--- exact place granular unit tests still earn their keep.
---
--- Layout is cargo-llvm-cov's OWN (no CARGO_TARGET_DIR override): builds isolate into
--- target/llvm-cov-target (never thrashing target/debug), profraws land at the MAIN target root
--- (show-env's LLVM_PROFILE_FILE), the cached profdata lives at target/llvm-cov-target/*.profdata.
--- Hand-rolling an extra isolation dir double-nested that layout and pointed the suite's
--- profraws where `report` never looks — three conducts read unit == blackbox == merged to
--- fourteen digits before this comment existed.

-- Where `report` actually reads profraws: the llvm-cov target dir's ROOT (nextest writes there).
-- show-env's LLVM_PROFILE_FILE points at the MAIN target root, which report never reads — a
-- census after a "identical three ways" conduct found 768 visible nextest profraws nested and
-- 444 invisible suite profraws at the main root. The suite's profile path is therefore pinned
-- into the scan root, and the unit stage lives OUTSIDE it so staged files are truly unseen.
local COV_DIR = prova.root .. "/target/llvm-cov-target"
local UNIT_STAGE = prova.root .. "/target/unit-profraws"

--- `cargo llvm-cov show-env` as a table (values are single-quoted).
local function cov_env()
  local r = shell.run({ "cargo", "llvm-cov", "show-env" }, { cwd = prova.root })
  local env = {}
  for line in (r.stdout or ""):gmatch("[^\n]+") do
    local k, v = line:match("^([%w_]+)='?([^']*)'?$")
    if k then env[k] = v end
  end
  return env
end

local function purge(dir, pat)
  for _, f in ipairs(fs.glob(dir, pat)) do
    fs.remove_all(f)
  end
end

--- Report over the profraws currently at the scan root. The cached profdata is purged first:
--- `report` reuses it and would silently ignore every profraw written since the last merge.
local function fresh_report()
  purge(COV_DIR, "*.profdata")
  local r = shell.run(
    { "cargo", "llvm-cov", "report", "--json" },
    { cwd = prova.root, timeout = "600s" }
  )
  return json.decode(r.stdout or "{}")
end

local function pct(rep)
  return rep.data[1].totals.lines.percent
end

--- Move every profraw at the scan root into the stage (or back out of it). Loud when a stage
--- that must move files moves none — a silent no-op staging is how three identical "layers"
--- passed for a merge, live.
local function stage(back)
  fs.mkdir(UNIT_STAGE)
  local from = back and UNIT_STAGE or COV_DIR
  local to = back and COV_DIR or UNIT_STAGE
  local moved = 0
  for _, f in ipairs(fs.glob(from, "*.profraw")) do
    shell.run({ "mv", f, to .. "/" }, { cwd = prova.root })
    moved = moved + 1
  end
  return moved
end

-- Conduct once: data-clean, instrumented build, the unit layer (reported alone), the black-box
-- layer (reported alone), the merge. DATA-only clean — the instrumented build artifacts are the
-- expensive stage and stay for incremental conducts. The suite must be green to be measured.
local conduct = prova.fixture("layered-coverage", Scope.File, function()
  local env = cov_env()
  -- A stale-generation guard: instrumented objects from a previous workspace version inflate the
  -- report's denominator (measured live: the 0.19.0 bump left 0.18.0 objects behind and both
  -- layers "regressed" by the same ~27% — a denominator artifact, not lost coverage). The stamp
  -- is this tree's own version; a mismatch wipes the whole coverage target before anything builds.
  local stamp = COV_DIR .. "/.prova-version-stamp"
  if not fs.exists(stamp) or fs.read(stamp) ~= prova.version then
    fs.remove_all(COV_DIR)
    fs.mkdir(COV_DIR)
    fs.write(stamp, prova.version)
  end
  purge(COV_DIR, "*.profraw")
  purge(COV_DIR, "*.profdata")
  purge(prova.root .. "/target", "*.profraw") -- strays from the misdirected show-env path
  purge(prova.root, "default_*.profraw") -- pre-run root strays (see the sweep below) are stale
  fs.remove_all(UNIT_STAGE)

  -- `--target-dir` is EXPLICIT because newer cargo-llvm-cov stopped setting CARGO_TARGET_DIR in
  -- show-env (it instruments via RUSTC_WRAPPER instead): without it this build lands the
  -- instrumented binary in target/debug — the TRAMPOLINE's binary — and every later `prova`
  -- invocation is silently instrumented, dropping a default_*.profraw into its cwd at exit
  -- (36 of them once reached the repo root and the snapshot). Isolation is the contract the
  -- header describes; this pin is what enforces it now that show-env no longer does.
  local build = shell.run({ "cargo", "build", "-p", "prova-cli", "--target-dir", COV_DIR },
    { cwd = prova.root, env = env, timeout = "1800s", merge_stderr = true })
  if build.code ~= 0 then
    return { error = "instrumented build failed:\n" .. (build.stdout or "") }
  end

  -- Layer 1, deputed: unit tests. Report alone, then stage its profraws aside.
  shell.run({ "cargo", "llvm-cov", "nextest", "--workspace", "--no-report" },
    { cwd = prova.root, timeout = "1800s" })
  local unit = fresh_report()
  if stage(false) == 0 then
    return { error = "staging moved no unit profraws — the scan-root assumption broke again" }
  end

  -- Layer 2, observed: the black-box suite through the instrumented binary. ONLY what
  -- instrumentation needs crosses in: LLVM_PROFILE_FILE (the %p pattern rides into every
  -- prova.bin child — the recursion is what gets measured) and PROVA_TRAMPOLINED (this IS this
  -- tree's build — skip the hop). Ambient cargo vars redirected sandbox proofs' builds, live.
  local suite = shell.run({ COV_DIR .. "/debug/prova" },
    { cwd = prova.root, timeout = "1200s", merge_stderr = true,
      env = { LLVM_PROFILE_FILE = COV_DIR .. "/suite-%p-%m.profraw", PROVA_TRAMPOLINED = "1" } })
  if suite.code ~= 0 then
    return { error = "the black-box suite is red under instrumentation — fix the bar before measuring it:\n"
      .. (suite.stdout or ""):sub(-2000) }
  end
  -- Sweep root strays INTO the scan root before reporting. An instrumented child whose
  -- environment lost LLVM_PROFILE_FILE falls back to `default_<sig>_0_<pid>.profraw` in its
  -- own cwd — LLVM's default, not a path prova controls. Same binary, same signature, so the
  -- data is mergeable: sweeping it in counts that coverage instead of littering the repo root
  -- (36 of these were once snapshotted into history before this sweep existed).
  for _, f in ipairs(fs.glob(prova.root, "default_*.profraw")) do
    shell.run({ "mv", f, COV_DIR .. "/" }, { cwd = prova.root })
  end
  local blackbox = fresh_report()

  -- The merge: unit profraws rejoin, one whole-bar total.
  stage(true)
  local merged = fresh_report()
  return { unit = unit, blackbox = blackbox, merged = merged }
end)

--- Per-file percent map from a full report.
local function by_file(rep)
  local out = {}
  for _, f in ipairs(rep.data[1].files or {}) do
    out[f.filename] = f.summary.lines.percent
  end
  return out
end

prova.test("whole-bar line coverage — unit AND black-box merged — does not regress past the baseline", {
  requires = { "cargo-llvm-cov", "cargo-nextest" },
  covers = "docs/design/verifiers.md#coverage-of-the-whole-bar",
  proves = "unit-only coverage read modules/socket.rs at 2% while it owned a whole proof directory — a number that misleads at the edges is worse than none; the merged total is the bar prova actually holds",
}, function(t)
  local produced = t:use(conduct)
  t:expect(produced.error, produced.error or "conduct produced reports"):is_nil()
  -- The merge is real or the gate is lying: identical-to-the-decimal layer totals mean a report
  -- read cached or half-invisible data (it happened three ways before this assert existed).
  t:expect(pct(produced.merged) == pct(produced.unit) and pct(produced.unit) == pct(produced.blackbox),
    "unit, blackbox, and merged are identical — the reports are not seeing distinct profraw sets"
  ):is_false()
  measure.ratchet(t, "rust.coverage.lines", pct(produced.merged), {
    set = "quality", direction = "higher_is_better",
  })
end)

prova.test("each layer's coverage holds on its own — and the delta names where unit tests are owed", {
  requires = { "cargo-llvm-cov", "cargo-nextest" },
  covers = "docs/design/verifiers.md#coverage-of-the-whole-bar",
  proves = "the delta is the signal: proven-black-box but unit-naked files are behavior with no fast local feedback — the granular-unit-test worklist, computed rather than guessed",
}, function(t)
  local produced = t:use(conduct)
  t:expect(produced.error, produced.error or "conduct produced reports"):is_nil()
  measure.ratchet(t, "rust.coverage.unit", pct(produced.unit), {
    set = "quality", direction = "higher_is_better",
  })
  measure.ratchet(t, "rust.coverage.blackbox", pct(produced.blackbox), {
    set = "quality", direction = "higher_is_better",
  })
  -- The worklist: files the proofs exercise heavily that unit tests barely touch.
  local unit_files = by_file(produced.unit)
  local rows = {}
  for file, bb in pairs(by_file(produced.blackbox)) do
    local u = unit_files[file] or 0
    if bb - u >= 40 then
      rows[#rows + 1] = { file = file, delta = bb - u, bb = bb, u = u }
    end
  end
  table.sort(rows, function(a, b) return a.delta > b.delta end)
  for i = 1, math.min(#rows, 10) do
    local r = rows[i]
    print(string.format("  unit-owed  %-60s black-box %5.1f%% · unit %5.1f%%",
      r.file:gsub(prova.root .. "/", ""), r.bb, r.u))
  end
end)
