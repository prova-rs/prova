--- The run record — what did NOT run, and the attestation over it.
---
--- "0 failed" is the last lie an *honest* agent can tell. It is technically true of a run in which
--- everything skipped, everything was deselected, or nothing was collected at all. Prova's own suite
--- reports `310 passed, 0 failed, 42 skipped` on this machine; 7 of those skips are placement specs
--- that need a broker nobody here is running. An agent reading only the exit code — or only the
--- failure count — reports that work as covered, and is not lying, and is wrong.
---
--- The record makes the negative space durable: per run, what executed and what did not, with the
--- skipped and deselected named rather than summed. `prova attest <address>` then answers the
--- question that actually matters about an obligation — not "did the suite pass" but "did the proof
--- for THIS claim actually execute" — and fails when it did not.
---
--- Written to `<home>/.prova/var/last-run.json` on every run (gitignored, prova's own footprint) and
--- emitted to an arbitrary path on demand with `--record <path>`, for CI to keep as an artifact. Not
--- signed: the threat model here is a careless agent, not a malicious one, and a signature would buy
--- ceremony rather than truth.

local OPEN = { spec = "the run record: not built (docs/plans/agent-reliability.md)" }

--- A package with two anchored claims: one discharged by a proof that runs, one by a proof that
--- cannot run here. That asymmetry is the whole subject — both proofs are green-by-absence-of-red,
--- and only one of them is evidence.
local sandbox = prova.fixture("record-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n\n[claims]\ndocs = ["docs"]\n')
  fs.write(proj .. "/docs/design.md", [[
# Design

<!-- claim: busy-not-absent -->
Contention and absence are different answers and must never be conflated.

<!-- claim: drain-not-preemption -->
A drain lets held leases finish; it never revokes one out from under its holder.
]])
  fs.write(proj .. "/proofs/contract_test.lua", [[
prova.test("busy is not unsatisfiable", {
  covers = "docs/design.md#busy-not-absent",
}, function(t)
  t:expect(1):equals(1)
end)

-- Discharges its claim on paper and cannot run on this machine. This is the shape the record
-- exists to expose: the suite is green, and this obligation has no evidence behind it.
prova.test("a drain is not a preemption", {
  covers = "docs/design.md#drain-not-preemption",
  requires = { "a-broker-nobody-here-runs" },
}, function(t)
  t:expect(1):equals(1)
end)
]])
  return proj
end)

prova.test("a run records what executed, and what did not", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })

  -- The record rides with the rest of prova's generated state, so it is gitignored and costs the
  -- user no edits to their own ignore files.
  local record = json.decode(fs.read(proj .. "/.prova/var/last-run.json"))

  t:expect(record.summary.passed, "the one proof that could run"):equals(1)
  t:expect(record.summary.failed):equals(0)
  t:expect(record.summary.skipped, "and the one that could not"):equals(1)
end)

prova.test("the skipped are named, with the reason they did not run", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local record = json.decode(fs.read(proj .. "/.prova/var/last-run.json"))

  -- A count is not enough. "42 skipped" and "42 skipped, all of them the thing you just claimed to
  -- have proven" are the same number and different facts, so the record names them individually.
  t:expect(#record.skipped):equals(1)
  t:expect(record.skipped[1].path):contains("a drain is not a preemption")
  t:expect(record.skipped[1].reason, "why it did not run"):contains("a-broker-nobody-here-runs")
end)

prova.test("the deselected are named too — never run is never run", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin .. " -k busy", { cwd = proj, merge_stderr = true })
  local record = json.decode(fs.read(proj .. "/.prova/var/last-run.json"))

  -- Deselection and skipping are different causes with one consequence: no evidence was produced.
  -- A selection is the easiest way of all to report green having tested nothing.
  t:expect(record.summary.deselected):equals(1)
  t:expect(json.encode(record.deselected)):contains("a drain is not a preemption")
end)

prova.test("--record also emits where asked, for CI to keep", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin .. " --record " .. proj .. "/run.json", { cwd = proj, merge_stderr = true })

  -- The var/ copy is for the next command; this one is for a human, an artifact upload, or a PR
  -- comment. Same content, somewhere durable.
  local emitted = json.decode(fs.read(proj .. "/run.json"))
  t:expect(emitted.summary.skipped):equals(1)
  t:expect(emitted.summary.passed):equals(1)
end)

prova.test("the record carries provenance, so a stale one is detectable", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local record = json.decode(fs.read(proj .. "/.prova/var/last-run.json"))

  -- Which binary produced it and how long it took. Without provenance a record is just an
  -- assertion, and re-reading yesterday's is indistinguishable from running today's.
  t:expect(record.binary, "the binary that produced it"):never():is_empty()
  t:expect(record.version):never():is_empty()
  t:expect(record.duration_ms):is_number()
end)

prova.test("attest passes when the covering proof actually executed", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest docs/design.md#busy-not-absent",
    { cwd = proj, merge_stderr = true })

  -- The honest case: an anchor, a proof that covers it, and a run in which that proof ran green.
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("busy-not-absent")
end)

prova.test("attest FAILS when the covering proof was skipped", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest docs/design.md#drain-not-preemption",
    { cwd = proj, merge_stderr = true })

  -- THE reason this exists. The suite exited 0. The claim is anchored, the binding is real, the
  -- proof is written — and it did not run, so nothing about this obligation was established.
  t:expect(r.code, "a claim with no evidence is not attested"):never():equals(0)
  t:expect(r.stdout):contains("drain-not-preemption")
  t:expect(r.stdout, "and says why"):contains("a-broker-nobody-here-runs")
end)

prova.test("attest FAILS when the covering proof was deselected", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin .. " -k busy", { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest docs/design.md#busy-not-absent",
    { cwd = proj, merge_stderr = true })
  local other = shell.run(prova.bin .. " attest docs/design.md#drain-not-preemption",
    { cwd = proj, merge_stderr = true })

  -- The selected claim still attests; the one filtered out does not. Narrowing a selection must
  -- narrow what can be claimed, or `-k` becomes a way to make any obligation green.
  t:expect(r.code, "the proof that ran still attests"):equals(0)
  t:expect(other.code, "the one filtered out does not"):never():equals(0)
end)

prova.test("attest FAILS when no run has been recorded at all", OPEN, function(t)
  local proj = t:use(sandbox)
  fs.remove_all(proj .. "/.prova/var")
  local r = shell.run(prova.bin .. " attest docs/design.md#busy-not-absent",
    { cwd = proj, merge_stderr = true })

  -- Absence of a record is absence of evidence. Treating "no record" as "fine" would make the
  -- whole atom opt-out by simply never running anything.
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("no run")
end)

prova.test("attest on an address no proof covers is refused, not passed", OPEN, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest docs/design.md#nobody-covers-this",
    { cwd = proj, merge_stderr = true })

  -- An address with no binding has no evidence by definition. Exiting 0 on "I found nothing to
  -- check" is the vacuous-pass shape this whole line of work exists to refuse.
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("nobody-covers-this")
end)

prova.test("recording is inert to the ordinary path", {
  proves = "the record is a byproduct, never a gate. This holds BEFORE the record exists and must still hold after — it is the regression guard on the ordinary path, not a spec, which is why prova refused to let it sit behind a spec flag",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })

  -- The record is a byproduct, never a gate: a run's verdict, output and exit code are exactly what
  -- they were before it existed. Machinery that changes the ordinary path gets switched off.
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("1 passed")
  t:expect(r.stdout):never():contains("attest")
end)

prova.test("the binary teaches the record: catalog, topic, and the verb", OPEN, function(t)
  local proj = t:use(sandbox)

  -- A capability an agent cannot discover does not exist, and discovery is two steps: the catalog
  -- names it, then the topic explains it.
  local catalog = shell.run(prova.bin .. " learn", { cwd = proj, merge_stderr = true })
  t:expect(catalog.code):equals(0)
  t:expect(catalog.stdout, "the catalog names the topic"):contains("record")

  local topic = shell.run(prova.bin .. " learn record", { cwd = proj, merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the verb"):contains("prova attest")
  t:expect(topic.stdout, "the flag"):contains("--record")
  t:expect(topic.stdout, "what it is FOR"):contains("skipped")
end)
