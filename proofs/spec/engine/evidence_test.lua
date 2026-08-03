--- `prova evidence` — the whole account — and `prova attest` with no address — the CI gate.
---
--- Until these existed, no verb answered "where does this project stand": `owed` showed only the
--- debts, and `attest` answered for one address at a time. `evidence` reports every stage of the
--- lifecycle with its count; bare `attest` folds every anchored claim into one exit code, which
--- is the only shape a pipeline can gate on.

local sandbox = prova.fixture("evidence-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n\n[claims]\ndocs = ["docs"]\n')
  fs.write(proj .. "/docs/design.md", [[
# Design

<!-- claim: busy-not-absent -->
Contention and absence are different answers.

<!-- claim: never-preempt -->
A held lease is never revoked.

<!-- claim: nobody-proves-me -->
An obligation with no covering proof.
]])
  fs.write(proj .. "/proofs/contract_test.lua", [[
prova.test("busy is not unsatisfiable", { covers = "docs/design.md#busy-not-absent" }, function(t)
  t:expect(1):equals(1)
end)

prova.test("a lease survives a drain", {
  covers = "docs/design.md#never-preempt",
  promises = "drain semantics need a multi-node broker",
}, function(t)
  t:expect(1):equals(2)
end)
]])
  return proj
end)

prova.test("evidence reports every stage of the account with its count", {
  covers = "docs/design/lifecycle.md#evidence-is-the-account",
  proves = "the whole-account view is the command the lifecycle was missing: owed shows only the debts and attest answers one address, so no verb could say where a project stands",
}, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " evidence", { cwd = proj, merge_stderr = true })

  t:expect(r.code, "a report, not a gate"):equals(0)
  -- The stages, named as the lifecycle names them, each with a count.
  t:expect(r.stdout, "claims found"):contains("CLAIMED")
  t:expect(r.stdout):contains("3")
  t:expect(r.stdout, "bindings found"):contains("BOUND")
  t:expect(r.stdout, "the open surface"):contains("PROMISED")
  t:expect(r.stdout, "reconciled against the record"):contains("ATTESTED")
  -- And the debts, so the report is actionable rather than a scoreboard.
  t:expect(r.stdout, "the unproven claim is named as owed"):contains("UNPROVEN")
end)

prova.test("evidence executes no proof body", {
  covers = "docs/design/lifecycle.md#two-verb-families",
  proves = "evidence joins the query family under the family's own contract: reading the account must be safe on any machine, whatever the proofs would do if run",
}, function(t)
  local proj = t:use(sandbox)
  local witness = proj .. "/BODY_RAN"
  fs.write(proj .. "/proofs/effect_test.lua", ([[
prova.test("has a side effect", function(t)
  fs.write(%q, "the body executed")
  t:expect(1):equals(1)
end)
]]):format(witness))
  shell.run(prova.bin .. " evidence", { cwd = proj, merge_stderr = true })
  local ran = fs.exists(witness)
  fs.remove_all(proj .. "/proofs/effect_test.lua")
  fs.remove_all(witness)

  t:expect(ran, "evidence must not run the body"):equals(false)
end)

prova.test("evidence without a recorded run says so instead of guessing", {
  proves = "an account that silently reports zero attested on a package that simply has not run yet reads as an indictment; absence of a record is a stated fact, not a zero",
}, function(t)
  local proj = t:use(sandbox)
  fs.remove_all(proj .. "/.prova/var")
  local r = shell.run(prova.bin .. " evidence", { cwd = proj, merge_stderr = true })

  t:expect(r.code):equals(0)
  t:expect(r.stdout, "the absence is stated"):contains("no run recorded")
end)

prova.test("bare attest reconciles every claim into one exit code", {
  covers = "docs/design/lifecycle.md#ci-can-ask-for-everything",
  proves = "an address at a time is a developer's question. The pipeline's question is whether everything this project claims is evidenced, and it has to be one exit code or CI cannot gate on it",
}, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest", { cwd = proj, merge_stderr = true })

  -- One claim attests (ran green), one is an open promise, one is unproven: NOT all attested.
  t:expect(r.code, "any unattested claim fails the gate"):never():equals(0)
  t:expect(r.stdout, "each claim gets its verdict"):contains("busy-not-absent")
  t:expect(r.stdout):contains("never-preempt")
  t:expect(r.stdout):contains("nobody-proves-me")
  t:expect(r.stdout, "with a tally"):contains("attested")
end)

prova.test("bare attest passes when every claim is evidenced", {
  proves = "the gate must be satisfiable, or it is a lecture rather than a bar — a package whose every claim has an executed, passing proof exits zero",
}, function(t)
  local proj = t:use(sandbox)
  -- Shrink the account to the one discharged, passing claim.
  fs.write(proj .. "/docs/design.md", [[
# Design

<!-- claim: busy-not-absent -->
Contention and absence are different answers.
]])
  fs.write(proj .. "/proofs/contract_test.lua", [[
prova.test("busy is not unsatisfiable", { covers = "docs/design.md#busy-not-absent" }, function(t)
  t:expect(1):equals(1)
end)
]])
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest", { cwd = proj, merge_stderr = true })

  t:expect(r.code, "everything claimed is evidenced"):equals(0)
end)

prova.test("bare attest on a package with no claims is a stated no-op", {
  proves = "exit 0 with a reason, not silence: a pipeline wiring the gate before declaring [claims] should learn it is gating nothing — and a package that never opted in must not fail for it",
}, function(t)
  local proj = t:use(sandbox)
  local bare = proj .. "/../bare-evidence"
  fs.mkdir(bare .. "/proofs")
  fs.write(bare .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(bare .. "/proofs/x_test.lua", 'prova.test("x", function(t) t:expect(1):equals(1) end)\n')
  shell.run(prova.bin, { cwd = bare, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest", { cwd = bare, merge_stderr = true })

  t:expect(r.code):equals(0)
  t:expect(r.stdout, "gating nothing is said out loud"):contains("no claims")
end)

prova.test("the binary teaches evidence: catalog, topic and the verbs", {
  proves = "a capability an agent cannot discover does not exist; the account verb is the entry point to the whole lifecycle, so its topic carries the family",
}, function(t)
  local proj = t:use(sandbox)
  local catalog = shell.run(prova.bin .. " learn", { cwd = proj, merge_stderr = true })
  t:expect(catalog.code):equals(0)
  t:expect(catalog.stdout, "the catalog names the topic"):contains("evidence")

  local topic = shell.run(prova.bin .. " learn evidence", { cwd = proj, merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the verb"):contains("prova evidence")
  t:expect(topic.stdout, "the CI gate"):contains("prova attest")
  t:expect(topic.stdout, "the narrowing"):contains("prova owed")
end)

prova.test("attest resolves a bare claim id when it is unambiguous", {
  proves = "the full address is a machine coordinate — an agent has it in its buffer, a human does not. Ids are memorable; making the unique ones resolve removes the copy/paste without loosening what an address means",
}, function(t)
  local proj = t:use(sandbox)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest busy-not-absent", { cwd = proj, merge_stderr = true })

  t:expect(r.code, "the unique id attests like its full address"):equals(0)
  t:expect(r.stdout, "resolved to the real address"):contains("docs/design.md#busy-not-absent")
end)

prova.test("an ambiguous bare id lists the candidates instead of guessing", {
  proves = "two docs may legally anchor the same id; picking either silently would attest something the caller did not ask about. Ambiguity is an answer with a menu, never a coin flip",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/docs/other.md", [[
# Other

<!-- claim: busy-not-absent -->
The same id, anchored in a second document.
]])
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest busy-not-absent", { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/docs/other.md")

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout, "named as ambiguous"):contains("ambiguous")
  t:expect(r.stdout, "both candidates are listed"):contains("docs/design.md#busy-not-absent")
  t:expect(r.stdout):contains("docs/other.md#busy-not-absent")
end)
