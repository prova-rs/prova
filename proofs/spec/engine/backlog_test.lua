--- Backlog — the cold state of a claim, and the muting that makes it safe to park in place.
---
--- A `<!-- backlog: id -->` anchor captures work in a doc without owing it. Backlog and claim are
--- the two states of one prose obligation: same shape, same id namespace, one keyword apart. The
--- backlog state is *muted* — out of `owed`, never failing `attest`, invisible to a bare run — so a
--- bug or a half-formed spec can be parked where it belongs without adding to what is owed right
--- now. `prova backlog promote <id>` thaws one into a claim, in place; the burndown sees it then and
--- not before.
---
--- The invariant that keeps the state machine legible: only a claim can be bound. A proof that
--- `covers` a still-cold item is reported (`BACKLOGGED`), never silently discharged.

--- A fresh, isolated project per test — `promote` writes to a doc, so nothing may be shared.
local function project(t, doc, proof)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(proj .. "/docs/design.md", doc)
  if proof then fs.write(proj .. "/proofs/contract_test.lua", proof) end
  return proj
end

local TWO_STATES = [[
# Design

<!-- claim: kept-promise -->
A held lease is never revoked out from under its holder.

<!-- backlog: flaky-teardown -->
Teardown occasionally leaves a container behind — investigate, someday.
]]

prova.test("a backlog item is muted from `owed`", {
  proves = "the whole point of the cold state: work parked in a doc that is being actively driven must not add to what that doc owes right now, or an agent mid-task is distracted by an obligation nobody asked it to take on",
}, function(t)
  local proj = project(t, TWO_STATES)
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- The claim is owed; the backlog item, sitting in the very same file, is not.
  t:expect(r.stdout, "the claim is owed"):contains("kept-promise")
  t:expect(r.stdout, "the backlog item is muted"):never():contains("flaky-teardown")
  t:expect(r.code, "owed reports, it does not gate"):equals(0)
end)

prova.test("`prova backlog` lists exactly what `owed` hides", {
  proves = "the cold shelf needs its own query — muting from `owed` would be a memory hole otherwise; the value of a human-driven lane is entirely in being able to review and promote it",
}, function(t)
  local proj = project(t, TWO_STATES)
  local r = shell.run(prova.bin .. " backlog", { cwd = proj, merge_stderr = true })

  t:expect(r.stdout, "the backlog item is listed"):contains("flaky-teardown")
  t:expect(r.stdout, "a claim is not a backlog item"):never():contains("kept-promise")
  t:expect(r.code):equals(0)
end)

prova.test("`promote` thaws a backlog item into a claim, in place", {
  proves = "promotion is a keyword flip, not a move: the id and its prose stay put so the diff reads as exactly 'this became active', and the address a future proof will name is already the one the reader sees",
}, function(t)
  local proj = project(t, TWO_STATES)

  local p = shell.run(prova.bin .. " backlog promote flaky-teardown", { cwd = proj, merge_stderr = true })
  t:expect(p.code):equals(0)
  t:expect(p.stdout):contains("backlog → claim")

  -- The anchor changed state in place; the id and prose did not move.
  local doc = fs.read(proj .. "/docs/design.md")
  t:expect(doc, "the keyword flipped"):contains("<!-- claim: flaky-teardown -->")
  t:expect(doc, "no backlog anchor remains for it"):never():contains("<!-- backlog: flaky-teardown -->")

  -- It is owed now — the burndown sees it — and off the cold shelf.
  local owed = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  t:expect(owed.stdout, "the promoted claim is now owed"):contains("flaky-teardown")
  t:expect(owed.stdout):contains("UNPROVEN")

  local backlog = shell.run(prova.bin .. " backlog", { cwd = proj, merge_stderr = true })
  t:expect(backlog.stdout, "it left the backlog"):never():contains("flaky-teardown")
end)

prova.test("a proof that covers a backlog item is BACKLOGGED, never silently discharged", {
  proves = "the load-bearing invariant of the state machine: only a claim can be bound. Letting a proof discharge a still-cold item would erase the distinction between 'promised to prove' and 'parked, undecided' — the two states would collapse",
}, function(t)
  local proj = project(t, [[
<!-- backlog: not-ready -->
A behaviour still being shaped — not yet promoted.
]], [[
prova.test("binds something still cold", {
  covers = "docs/design.md#not-ready",
}, function(t)
  t:expect(1):equals(1)
end)
]])
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  t:expect(r.stdout, "the misuse is named"):contains("BACKLOGGED")
  t:expect(r.stdout, "and the one-keyword remedy is given"):contains("promote")
  t:expect(r.code, "still a report, not a gate"):equals(0)
end)

prova.test("the CI gate does not fail on a parked backlog item", {
  proves = "`prova attest` gates claims; a backlog item is unbound by definition, so gating on it would turn 'I parked a bug in this doc' into a red pipeline — the exact opposite of the point",
}, function(t)
  local proj = project(t, [[
<!-- claim: kept-promise -->
A held lease is never revoked.

<!-- backlog: parked-bug -->
A rough edge worth fixing later.
]], [[
prova.test("the lease holds", {
  covers = "docs/design.md#kept-promise",
}, function(t)
  t:expect(1):equals(1)
end)
]])
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })  -- record a run
  local r = shell.run(prova.bin .. " attest", { cwd = proj, merge_stderr = true })

  -- The one claim attests; the backlog item is not among the things gated at all.
  t:expect(r.code, "the gate passes"):equals(0)
  t:expect(r.stdout, "the backlog item is not counted as an unattested claim"):never():contains("parked-bug")
end)

prova.test("a bare run ignores the backlog entirely", {
  proves = "prova must not parse markdown to run a test, and a parked backlog item must never colour a run — reconciliation belongs only to the verbs that ask for it",
}, function(t)
  local proj = project(t, TWO_STATES, [[
prova.test("unrelated", function(t) t:expect(1):equals(1) end)
]])
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })

  t:expect(r.stdout):never():contains("flaky-teardown")
  t:expect(r.stdout):never():contains("BACKLOGGED")
end)

prova.test("a claim and a backlog item cannot share an id in one file", {
  proves = "one id namespace is what makes promotion a safe in-place flip; two anchors carrying one id — whichever states they are in — form an ambiguous address, the same defect a duplicate claim is",
}, function(t)
  local proj = project(t, [[
<!-- claim: twice -->
First.

<!-- backlog: twice -->
Second.
]])
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  t:expect(r.code, "an ambiguous address is a defect, not unfinished work"):never():equals(0)
  t:expect(r.stdout):contains("twice")
end)

prova.test("the binary teaches the backlog: catalog and topic", {
  proves = "a capability an agent cannot discover does not exist. The catalog must name the lane and the topic must explain the anchor, the muting, and the one write — checking one leaves the capability either unfindable or unexplained",
}, function(t)
  local proj = project(t, TWO_STATES)

  local catalog = shell.run(prova.bin .. " learn", { cwd = proj, merge_stderr = true })
  t:expect(catalog.stdout, "the catalog names the topic"):contains("backlog")

  local topic = shell.run(prova.bin .. " learn backlog", { cwd = proj, merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the anchor form"):contains("<!-- backlog:")
  t:expect(topic.stdout, "the verb"):contains("prova backlog")
  t:expect(topic.stdout, "the promotion"):contains("promote")
end)
