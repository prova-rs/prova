-- The workspace's deputies — shared conduct recipes (docs/plans/shared-deputies.md).
-- `require("deputies")` from any suite: each conduct is Scope.Run — declared here once, conducted
-- at most once per RUN whatever the outcome, its account readable from any suite on any worker.
--
-- House rule riding along: a leaf that reads a cargo-conducting deputy declares
-- `locks = { prova.writes("cargo") }`. The first `t:use` is what runs cargo, so any leaf that
-- MIGHT conduct must hold the token the conduct contends on.
local M = {}

-- One `cargo nextest` of the whole workspace, emitting the junit artifact every reader parses.
-- Liveness-supervised, not wall-priced (verifiers.md#conduct-heartbeat-not-deadline) — but
-- priced honestly: cargo is chatty while TESTS run, yet a single big crate's codegen is SILENT
-- for minutes (observed live: a 120s idle bound killed a healthy compile of prova-cli), so the
-- believable-silence window here is the longest quiet compile stretch, not a heartbeat. The
-- stale artifact is removed FIRST, so a deputy that dies before emitting leaves nothing behind
-- and the adoption fails loudly on "matched nothing" — never a previous run's verdicts wearing
-- this run's face.
M.nextest = prova.fixture("nextest-junit", Scope.Run, function()
  local artifact = prova.root .. "/target/nextest/prova/junit.xml"
  fs.remove_all(artifact)
  local cmd = { "cargo", "nextest", "run", "--workspace", "--profile", "prova" }
  -- Pushdown (verifiers.md#selection-pushdown-into-conducts): the run's `-k` keywords narrow
  -- the conduct to matching case names — one selection vocabulary across granularities, on the
  -- convention that the keyword appears in both the reader proof's name and the deputed case's.
  -- Only clean identifier-shaped keywords ride (anything else could bend nextest's expression
  -- grammar — conducting FULL is the safe over-approximation); tags and nodes are prova-side
  -- vocabularies nextest cannot speak, so they never narrow. The adopting run records itself
  -- as a NARROWED account, so attest never vouches for what the narrowing excluded.
  for _, k in ipairs(prova.selection.keywords) do
    if k:match("^[%w_:-]+$") then
      cmd[#cmd + 1] = "-E"
      cmd[#cmd + 1] = "test(" .. k .. ")"
    end
  end
  shell.run(cmd, { cwd = prova.root, merge_stderr = true, idle_timeout = "600s", timeout = "1800s" })

  -- CUSTODY (docs/design/verifiers.md#reports-are-custody-not-visualization). The account already
  -- adopts every CASE from this artifact; the artifact itself lives under `target/`, which the
  -- sweep deletes — so the detail behind a red case (stdout, the failure message, timings) went
  -- away with it. Publishing keeps it addressable: `prova reports unit-cases --kind xml`.
  --
  -- One form, because nextest emits one. A report does not owe every reader a bespoke rendering —
  -- it owes them an honest list of what exists, and junit XML is what this deputy produced.
  if fs.exists(artifact) then
    local adopted = junit.load(artifact)
    report.publish{
      name = "unit-cases",
      summary = string.format("%d cases · %d passed · %d failed · %d skipped",
        adopted.total, adopted.passed, adopted.failed + adopted.errors, adopted.skipped),
      forms = { xml = artifact },
    }
  end

  return artifact
end)

return M
