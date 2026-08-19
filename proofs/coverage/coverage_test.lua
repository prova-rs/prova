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
-- into the scan root, and the staging dir lives OUTSIDE it so staged files are truly unseen.
-- WHERE THE FLOORS SIT, and why they are not the high-water mark (re-banked 2026-08-16).
--
-- A ratchet banked at a peak fails forever after. The 2026-08-11 floors were set as the closing
-- act of a push whose whole purpose was raising them — the most favourable measurement the tree
-- had ever produced — and CI went red within a day, stayed red for five, and cost two sessions
-- arguing about a number instead of reading it.
--
-- So each coverage floor now carries a BAND, and the trigger sits a few degrees back from the
-- edge. The band is not slack to be spent; it is the distance at which a trip still has obvious
-- material behind it. Trip a ratchet 1pp down and the unit-owed worklist below names files with
-- 40+ point deltas — real work, chosen by consequence. Trip it at 0.01pp and the only moves left
-- are the ones that damage the codebase: tests that execute lines without asserting behavior,
-- written to move a number rather than to catch a defect.
--
-- The bands are sized from measured behavior, not taste:
--
--   unit      1.0  Was ZERO, and that is what proved the point: two runs of IDENTICAL code
--                  measured 21043 and 21040 covered lines, and the 0.0019pp difference failed the
--                  release gate. The metric has run-to-run jitter, so a zero-tolerance floor gates
--                  releases on noise. 1.0pp is also roughly a week of this tree's feature velocity
--                  (four days of features diluted it ~0.76pp), so ordinary work does not trip it.
--   blackbox  1.0  Unchanged — it was already the one band sized for a layer whose denominator
--                  moves when instrumented objects enter or leave the scan.
--   merged    0.5  Half the others on purpose: the union of both layers moves LESS, because a
--                  proof-first tree covers new behavior black-box as it lands.
--
-- Raising a floor is `--update-baseline` (it tightens only, and refuses to loosen). LOWERING one
-- is a hand edit of .prova/baselines/quality.json, deliberately — with the reason in the commit,
-- never as a way past a red gate.
local COV_DIR = prova.root .. "/target/llvm-cov-target"
local SUITE_STAGE = prova.root .. "/target/suite-profraws"
local EXEC_STAGE = prova.root .. "/target/exec-stage"

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

--- What coverage is measured ABOUT: shipped code. `xtask` is this repo's build automation — it
--- runs on a developer's machine to produce artifacts, it is never in a release, and no user can
--- reach it. Measuring it puts 88 lines a test suite has no business exercising into the
--- denominator of every layer, which is not a coverage gap but a category error.
---
--- Scoped deliberately and on its own merits, not as a route past a red ratchet: it is worth
--- ~+0.23pp, the gap it was decided against was ~217 lines, and the floors are re-banked against
--- the new denominator in the same commit so nothing is credited twice.
local IGNORE_FILES = "(^|/)xtask/"

--- Report over the profraws currently at the scan root. The cached profdata is purged first:
--- `report` reuses it and would silently ignore every profraw written since the last merge.
local function fresh_report()
  purge(COV_DIR, "*.profdata")
  local r = shell.run(
    { "cargo", "llvm-cov", "report", "--json", "--ignore-filename-regex", IGNORE_FILES },
    { cwd = prova.root, timeout = "600s" }
  )
  return json.decode(r.stdout or "{}")
end

local function pct(rep)
  return rep.data[1].totals.lines.percent
end

--- Move the linked executables out of `deps/` (or back). `report` derives its denominator from
--- every instrumented object it can scan, so the previous conduct's TEST binaries must be out of
--- sight while the black-box layer reports (measured live: their `#[cfg(test)]` code cost that
--- layer 8.2 points — 68.9% read as 60.7%). Extensionless files in deps are the linked
--- executables; the .rlib/.d fingerprint artifacts that drive incremental compiles stay put, so
--- the round trip costs one prova relink and nothing else. Moving back never clobbers: a fresh
--- artifact under a staged name (the just-relinked prova bin) wins and the stale copy is dropped.
local function stage_execs(back)
  fs.mkdir(EXEC_STAGE)
  local deps = COV_DIR .. "/debug/deps"
  local from = back and EXEC_STAGE or deps
  local to = back and deps or EXEC_STAGE
  for _, f in ipairs(fs.glob(from, "*")) do
    if not f:match("%.%w+$") then
      local dest = to .. "/" .. f:match("([^/]+)$")
      if fs.exists(dest) then
        fs.remove_all(f)
      else
        shell.run({ "mv", f, dest }, { cwd = prova.root })
      end
    end
  end
end

--- Move every profraw at the scan root into the stage (or back out of it). Loud when a stage
--- that must move files moves none — a silent no-op staging is how three identical "layers"
--- passed for a merge, live.
local function stage(back)
  fs.mkdir(SUITE_STAGE)
  local from = back and SUITE_STAGE or COV_DIR
  local to = back and COV_DIR or SUITE_STAGE
  local moved = 0
  for _, f in ipairs(fs.glob(from, "*.profraw")) do
    shell.run({ "mv", f, to .. "/" }, { cwd = prova.root })
    moved = moved + 1
  end
  return moved
end

-- Conduct once: data-clean, instrumented build, the black-box layer (reported alone, against
-- the shipping binary only), the unit layer (reported alone), the merge. DATA-only clean — the
-- instrumented build artifacts are the expensive stage and stay for incremental conducts. The
-- suite must be green to be measured.
local conduct = prova.fixture("layered-coverage", Scope.File, function()
  local env = cov_env()
  -- A stale-generation guard: instrumented objects from a previous workspace version inflate the
  -- report's denominator (measured live: the 0.19.0 bump left 0.18.0 objects behind and both
  -- layers "regressed" by the same ~27% — a denominator artifact, not lost coverage). The stamp
  -- is this tree's own version; a mismatch wipes the whole coverage target before anything builds.
  local stamp = COV_DIR .. "/.prova-version-stamp"
  if not fs.exists(stamp) or fs.read(stamp) ~= prova.version then
    fs.remove_all(COV_DIR)
    fs.remove_all(EXEC_STAGE) -- staged executables are the wiped generation's — stale with it
    fs.mkdir(COV_DIR)
    fs.write(stamp, prova.version)
  end
  purge(COV_DIR, "*.profraw")
  purge(COV_DIR, "*.profdata")
  purge(prova.root .. "/target", "*.profraw") -- strays from the misdirected show-env path
  purge(prova.root, "default_*.profraw") -- pre-run root strays (see the sweep below) are stale
  fs.remove_all(SUITE_STAGE)
  stage_execs(true) -- an aborted conduct leaves executables staged; restore before building

  -- The previous conduct's nextest executables leave the scan before the black-box layer reports
  -- (see `stage_execs`); the build below then relinks the one executable that layer measures.
  stage_execs(false)

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

  -- Layer 1, observed: the black-box suite through the instrumented binary — reported BEFORE
  -- the unit stage builds anything, deliberately. `report` derives its denominator from every
  -- instrumented object in the target dir, so once nextest's test binaries exist, the black-box
  -- layer pays denominator rent for `#[cfg(test)]` code it can never execute — measured live:
  -- each unit-test batch sank the black-box percent (~0.2–1.2%/batch) until the layer breached
  -- its own tolerance band with no behavior change anywhere. Suite-first, the black-box
  -- denominator is the shipping code alone. ONLY what instrumentation needs crosses in (ambient
  -- cargo vars redirected sandbox proofs' builds, live):
  --
  --   LLVM_PROFILE_FILE  the %p pattern rides into every prova.bin child.
  --   PROVA_SUBJECT_BIN  makes those children the INSTRUMENTED build. The recursion is where the
  --                      runtime executes, so it is most of what this layer measures — and until
  --                      2026-08-16 it was measuring none of it. The variable this replaces,
  --                      `PROVA_TRAMPOLINED`, was read by nothing: it named a re-exec mechanism
  --                      that had since been retired, so `prova.bin` resolved to the declared
  --                      [runner] (the ordinary uninstrumented target/debug/prova) and every child
  --                      contributed zero. The layer read 45% against a 69% floor with no coverage
  --                      lost, and the ratchet was blamed for four days.
  local suite = shell.run({ COV_DIR .. "/debug/prova" },
    { cwd = prova.root, timeout = "1200s", merge_stderr = true,
      env = { LLVM_PROFILE_FILE = COV_DIR .. "/suite-%p-%m.profraw",
              PROVA_SUBJECT_BIN = COV_DIR .. "/debug/prova" } })
  if suite.code ~= 0 then
    -- The tail of this capture is NOT the failure. A run's summary is the last thing the runner
    -- prints, but detached children (`--reprovision`, backgrounded with `&`) hold the same pipe
    -- open and keep narrating after the parent exits — so the final bytes are reliably progress
    -- noise, and a `:sub(-2000)` of it showed twelve identical "running … done in 1.7s" lines and
    -- not one word about what failed. Measured: three consecutive conducts were diagnosed blind
    -- because of it. Select the reporter's own verdict lines instead, and keep the whole capture.
    local log = prova.root .. "/target/coverage-suite.log"
    fs.write(log, suite.stdout or "")
    local verdicts = {}
    for line in (suite.stdout or ""):gmatch("[^\n]+") do
      if line:match("FAIL") or line:match("%d+ passed,") then
        verdicts[#verdicts + 1] = line
      end
    end
    if #verdicts == 0 then
      verdicts[1] = "(the suite printed no verdict line at all — it died rather than reported)"
    end
    return { error = "the black-box suite is red under instrumentation — fix the bar before measuring it:\n"
      .. table.concat(verdicts, "\n") .. "\n  full capture: " .. log }
  end
  -- Sweep root strays INTO the scan root before reporting. An instrumented child whose
  -- environment lost LLVM_PROFILE_FILE falls back to `default_<sig>_0_<pid>.profraw` in its
  -- own cwd — LLVM's default, not a path prova controls. Same binary, same signature, so the
  -- data is mergeable: sweeping it in counts that coverage instead of littering the repo root
  -- (36 of these were once snapshotted into history before this sweep existed).
  for _, f in ipairs(fs.glob(prova.root, "default_*.profraw")) do
    shell.run({ "mv", f, COV_DIR .. "/" }, { cwd = prova.root })
  end

  -- Did the RECURSION get measured? This layer's number is mostly what `prova.bin` children
  -- executed, so one profraw per instrumented process is the evidence that the thing being
  -- reported is the thing intended. When the subject silently reverted to the uninstrumented
  -- build, the suite still passed, the report still produced a number, and the only visible
  -- symptom was 24 points of "regression" that no code change explained — for four days. Two
  -- profraws from a 197-second suite was the fact that named it, and nothing was checking.
  --
  -- The floor is deliberately far below a healthy conduct (hundreds) and far above the broken
  -- one (2): this is a did-it-happen-at-all guard, not a count to tune.
  local recursion = #fs.glob(COV_DIR, "suite-*.profraw")
  if recursion < 20 then
    return { error = string.format(
      "the black-box layer measured only its own conductor: %d suite profraw(s). The subject is "
      .. "not instrumented, so every prova.bin child contributed nothing — check that "
      .. "PROVA_SUBJECT_BIN reaches the children and that %s/debug/prova is the build they run.",
      recursion, COV_DIR) }
  end

  local blackbox = fresh_report()
  if stage(false) == 0 then
    return { error = "staging moved no suite profraws — the scan-root assumption broke again" }
  end
  stage_execs(true) -- the black-box layer has reported; nextest reuses these instead of relinking

  -- Layer 2, deputed: unit tests. Reported alone (the suite's profraws are staged aside); the
  -- test binaries this builds join the denominator from here on, which is honest for a layer
  -- whose own tests are what run.
  shell.run({ "cargo", "llvm-cov", "nextest", "--workspace", "--no-report" },
    { cwd = prova.root, timeout = "1800s" })
  local unit = fresh_report()

  -- The merge: suite profraws rejoin, one whole-bar total.
  stage(true)
  local merged = fresh_report()

  -- CUSTODY (docs/design/verifiers.md#reports-are-custody-not-visualization). The conduct has
  -- produced the answer to "which lines"; without this it is discarded, and the floor can refuse a
  -- regression while being unable to show what moved. That is not hypothetical — it is what made
  -- diagnosing this layer cost days.
  --
  -- Two forms of one fact, because the two readers differ: llvm-cov's own HTML for a person, the
  -- merged JSON for an agent (and for diffing two runs). Prova renders neither — llvm-cov does, and
  -- prova takes custody so `target/` being swept does not take the evidence with it.
  local html_dir = prova.root .. "/target/coverage-html"
  fs.remove_all(html_dir)
  local html = shell.run(
    { "cargo", "llvm-cov", "report", "--html", "--output-dir", html_dir,
      "--ignore-filename-regex", IGNORE_FILES },
    { cwd = prova.root, timeout = "600s" })
  local json_path = prova.root .. "/target/coverage-merged.json"
  fs.write(json_path, json.encode(merged))

  local forms = { json = json_path }
  -- llvm-cov writes a TREE at <output-dir>/html — a page per file, linked from an index. The whole
  -- tree is published (custody copies it and addresses its index.html), because copying the entry
  -- point alone would file a report with every link broken.
  --
  -- The HTML pass is one extra llvm-cov invocation over profdata that already exists. If it fails,
  -- the JSON still publishes: a missing human form is worth less than the whole report going away.
  if html.code == 0 and fs.exists(html_dir .. "/html/index.html") then
    forms.html = html_dir .. "/html"
  end

  report.publish{
    name = "coverage",
    summary = string.format("unit %.2f%% · black-box %.2f%% · merged %.2f%% (%d/%d lines)",
      pct(unit), pct(blackbox), pct(merged),
      merged.data[1].totals.lines.covered, merged.data[1].totals.lines.count),
    -- Named so a red ratchet can point at the evidence instead of leaving the reader to rebuild
    -- the conduct: these three are exactly the floors this artifact explains.
    explains = { "rust.coverage.unit", "rust.coverage.blackbox", "rust.coverage.lines" },
    forms = forms,
  }

  return { unit = unit, blackbox = blackbox, merged = merged }
end)


--- WHAT EACH LAYER WAS MEASURED AGAINST, banked beside what it measured.
---
--- A percentage is a fraction, and this conduct only ever banked the numerator's share. The
--- denominator — how many lines `cargo llvm-cov report` counted — is a property of the OBJECT
--- POPULATION in the scan dir at report time, not of the source: `report` takes no `--bin`/`--lib`
--- scoping, so it counts whatever instrumented artifacts happen to be there, and a later build can
--- evict or replace what an earlier report was measured against.
---
--- Measured 2026-08-19, four clean-slate conducts: the commit that BANKED the 86.37% floor
--- re-measures at 65.74% and counts 35,514 lines, where a strictly larger tree three days later
--- counts 29,824. The source did not shrink. Nothing regressed; the basis moved, and a ratchet
--- comparing across a moved basis reports a twenty-point fiction with total confidence.
---
--- This does not make the denominator stable — that is llvm-cov's to give and it does not. It
--- makes a moved basis LOUD and names it, so the failure says "your instrument changed" instead of
--- "your coverage collapsed", and so a floor can never again be banked against a basis nobody
--- checked. The header above records two earlier incidents diagnosed by hand, over four days and
--- three conducts; both would have been one line of output with this in place.
---
--- Re-banking is deliberate and reviewable: edit the table below in the same commit that edits the
--- floors, because the two are one measurement and must move together.
---
--- **Unbanked as of 2026-08-19, and honestly so.** Banking a basis requires one conduct that
--- completes, and the lane currently cannot finish
--- (agent-ergonomics.md#coverage-lane-blocked-by-a-contended-timing-proof). Until then this guard
--- reports each layer's measured basis and says it is unbanked — it does not fail the run for a
--- baseline nobody has been able to establish. The first conduct that completes prints the three
--- numbers to put here.
local BASIS = {
  -- layer      lines counted at bank time
  unit     = nil,
  blackbox = nil,
  merged   = nil,
}

--- How far the basis may drift before the percentages stop being comparable. Small on purpose:
--- codegen jitter is fractions of a percent, and the incidents this exists for moved it by 15-20%.
local BASIS_TOLERANCE = 0.02

--- Check a layer's basis against its bank. Returns nil when it holds, else the sentence to fail on.
local function basis_drift(layer, rep)
  local banked = BASIS[layer]
  local measured = rep.data[1].totals.lines.count
  if not banked then
    return nil, string.format(
      "  basis      %-9s %6d lines measured, NOT BANKED — bank it in coverage_test.lua's BASIS "
      .. "table alongside the floor", layer, measured)
  end
  local drift = math.abs(measured - banked) / banked
  if drift <= BASIS_TOLERANCE then
    return nil, nil
  end
  return string.format(
    "the %s layer's measurement basis moved: %d lines counted, %d banked (%+.1f%%). A percentage "
    .. "measured against a different basis is NOT comparable to a floor measured against this one "
    .. "— `cargo llvm-cov report` counts whatever instrumented objects are in the scan dir, so the "
    .. "denominator is build state, not source. Re-measure and re-bank the floor AND the basis "
    .. "together (coverage_test.lua's BASIS table), or find what changed the object population.",
    layer, measured, banked, 100 * (measured - banked) / banked)
end

--- Per-file percent map from a full report.
local function by_file(rep)
  local out = {}
  for _, f in ipairs(rep.data[1].files or {}) do
    out[f.filename] = f.summary.lines.percent
  end
  return out
end

prova.test("whole-bar line coverage — unit AND black-box merged — does not regress past the baseline", {
  locks = { prova.writes("cargo") },
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
  -- Before the ratchet, never after: a moved basis makes the percentage incomparable, and
  -- reporting that as a coverage regression is the exact fiction this guard exists to stop.
  local drift, note = basis_drift("merged", produced.merged)
  if note then print(note) end
  t:expect(drift, drift or "the merged layer's basis is the one the floor was banked against")
    :is_nil()

  measure.ratchet(t, "rust.coverage.lines", pct(produced.merged), {
    set = "quality", direction = "higher_is_better",
  })
end)

prova.test("each layer's coverage holds on its own — and the delta names where unit tests are owed", {
  locks = { prova.writes("cargo") },
  requires = { "cargo-llvm-cov", "cargo-nextest" },
  covers = "docs/design/verifiers.md#coverage-of-the-whole-bar",
  proves = "the delta is the signal: proven-black-box but unit-naked files are behavior with no fast local feedback — the granular-unit-test worklist, computed rather than guessed",
}, function(t)
  local produced = t:use(conduct)
  t:expect(produced.error, produced.error or "conduct produced reports"):is_nil()

  -- The exclusion is a regex in a shell argument — a typo silently measures everything again and
  -- the floors drift back down for a reason nobody can see. Assert the denominator directly.
  local measured_xtask = {}
  for file in pairs(by_file(produced.merged)) do
    if file:match("/xtask/") then measured_xtask[#measured_xtask + 1] = file end
  end
  t:expect(measured_xtask, "build automation is not shipped code and is not in the denominator")
    :has_length(0)

  -- Each layer's DENOMINATOR, printed before any ratchet fires. A percent alone cannot be
  -- diagnosed: this proof's own header records two separate incidents where a layer moved several
  -- points with no behavior change anywhere, because instrumented objects entered or left the scan
  -- and the denominator moved underneath it. Printed BEFORE the assertions on purpose — a failing
  -- ratchet aborts the body, so anything printed after it is missing on exactly the runs that need
  -- it (the unit-owed worklist below has been invisible for days for this reason).
  for _, layer in ipairs({ { "unit", produced.unit }, { "blackbox", produced.blackbox },
                           { "merged", produced.merged } }) do
    local tot = layer[2].data[1].totals.lines
    print(string.format("  layer %-9s %6.2f%%  %6d/%-6d lines  %4d files",
      layer[1], tot.percent, tot.covered, tot.count, #(layer[2].data[1].files or {})))
  end

  for _, layer in ipairs({ { "unit", produced.unit }, { "blackbox", produced.blackbox } }) do
    local drift, note = basis_drift(layer[1], layer[2])
    if note then print(note) end
    t:expect(drift, drift or layer[1] .. "'s basis is the one its floor was banked against")
      :is_nil()
  end

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
