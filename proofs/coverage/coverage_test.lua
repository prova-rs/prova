--- Code coverage as a ratcheted quality gate (the endgame leg of
--- docs/design/verifiers.md#exclusive-quality-interface): conduct `cargo llvm-cov nextest` once —
--- an instrumented build + run of the whole workspace's unit tests — read the line-coverage
--- total, and hold it to the committed baseline. Lower-than-baseline is red; a genuine new floor
--- is raised deliberately via `prova run coverage --update-baseline`, never by drift.
---
--- `cargo-llvm-cov` is a world fact (`requires`/`must_run`); asking for coverage is intent (the
--- `coverage` switch, suite.lua) — two facts, two remedies, as everywhere.

-- Conduct once: the JSON summary rides stdout; the tests run instrumented underneath. The
-- deputy's exit code is not asserted — a failing unit test is the ut leg's report; this gate
-- answers only for coverage (and a died-before-emitting deputy fails the parse loudly below).
local conduct = prova.fixture("llvm-cov-summary", Scope.File, function()
  local r = shell.run(
    { "cargo", "llvm-cov", "nextest", "--workspace", "--json", "--summary-only" },
    { cwd = prova.root, timeout = "1800s" }
  )
  return r.stdout or ""
end)

prova.test("workspace line coverage does not regress past the baseline", {
  requires = { "cargo-llvm-cov", "cargo-nextest" },
}, function(t)
  local report = json.decode(t:use(conduct))
  local percent = report.data[1].totals.lines.percent
  t:expect(percent, "the summary carries a line-coverage total"):gte(0)
  measure.ratchet(t, "rust.coverage.lines", percent, {
    set = "quality",
    direction = "higher_is_better",
  })
end)
