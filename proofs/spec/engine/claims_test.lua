--- Claims — binding prose to proofs, and reporting what is owed.
---
--- Specs do not only come from prova. They come from design docs, tickets, and conversations, and
--- an agent can say it implemented one without ever having done so. A `<!-- claim: id -->` anchor
--- in prose is an obligation entering the system from outside; `covers = "path#id"` on a proof is
--- the discharge. What prova adds is the reconciliation: which obligations exist, which are
--- discharged, and which references point at nothing.
---
--- Opt-in at the package level: no `[specs]` section means no scanning and no cost. Reported, never
--- fatal — except a duplicate id, which makes an address ambiguous and is a defect rather than
--- unfinished work.

local sandbox = prova.fixture("claims-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  local proj = root .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(proj .. "/docs/design.md", [[
# Design

<!-- claim: busy-not-absent -->
Contention and absence are different answers and must never be conflated.

<!-- claim: never-preempt -->
A held lease is never revoked out from under its holder.

<!-- claim: nobody-proves-me -->
This claim has no covering proof and should be reported as owed.
]])
  fs.write(proj .. "/proofs/contract_test.lua", [[
prova.test("busy is not unsatisfiable", {
  covers = "docs/design.md#busy-not-absent",
}, function(t)
  t:expect(1):equals(1)
end)

prova.test("a lease survives a drain", {
  covers = "docs/design.md#never-preempt",
  promises = "drain semantics need a multi-node broker",
}, function(t)
  t:expect(1):equals(2)
end)

prova.test("points at prose nobody wrote", {
  covers = "docs/design.md#not-written-yet",
}, function(t)
  t:expect(1):equals(1)
end)
]])
  return proj
end)

prova.test("`prova owed` reports an anchored claim with no covering proof", {
  proves = "the intake half: writing an anchor admits an obligation from outside prova entirely, so work scoped in prose stops being invisible until someone remembers it",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- The intake half: writing an anchor admits an obligation from outside prova entirely, and it
  -- shows up as work owed rather than being invisible until someone remembers it.
  t:expect(r.stdout, "the unproven claim is owed"):contains("nobody-proves-me")
  t:expect(r.stdout):contains("UNPROVEN")
end)

prova.test("a covered claim is not reported as owed", {
  proves = "discharged obligations must leave the list, or the list stops being read",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- Discharged obligations must leave the list, or the list stops being read.
  t:expect(r.stdout):never():contains("busy-not-absent")
end)

prova.test("a covers pointing at no anchor is UNBOUND, and not fatal", {
  proves = "prose-not-yet-written and prose-deleted produce the same state and want different remedies; both are unfinished work and neither is a broken build",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- Two situations produce this identical state — prose not written yet, and prose deleted after
  -- the proof captured the contract. Both are unfinished work, neither is a broken build, and the
  -- remedy differs (write it, or retire the reference into `proves`).
  t:expect(r.stdout):contains("DANGLING")
  t:expect(r.stdout):contains("not-written-yet")
  t:expect(r.code, "owed reports, it does not gate"):equals(0)
end)

prova.test("open specs and unproven claims share one list", {
  proves = "an agent orienting in a repo asks one question — what is owed here? An answer living in two places has one that goes stale",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- An agent orienting in a repo asks ONE question — what is owed here? Origin is a column, not a
  -- separate concept, or the answer lives in two places and one of them goes stale.
  t:expect(r.stdout, "the open promise is owed too"):contains("PROMISED")
  t:expect(r.stdout):contains("multi-node broker")
end)

prova.test("a duplicate claim id in one file is an error", {
  proves = "unlike everything else here this is a real defect: an ambiguous address cannot be discharged by anything, so the ledger is incoherent rather than behind",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/docs/dupe.md", [[
<!-- claim: twice -->
First.

<!-- claim: twice -->
Second.
]])
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/docs/dupe.md")

  -- Unlike everything else here, this is a real defect: an ambiguous address cannot be discharged
  -- by anything, so the system is incoherent rather than merely behind.
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("twice")
end)

prova.test("a malformed anchor is a helpful error, not a silent disappearance", {
  proves = "the keyword is the line of intent — a `<!-- backlog: … -->` the author wrote must never vanish into prose; a typo'd date says WHY at file:line, or it becomes a thing hunted for and never found. Prose without the keyword stays invisible; this is the difference",
}, function(t)
  local proj = t:use(sandbox)
  fs.write(proj .. "/docs/oops.md", "<!-- backlog: flaky-teardown 2026-13-45 -->\nAn impossible month.\n")
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/docs/oops.md")

  t:expect(r.code, "a defect, so it errors like a duplicate id"):never():equals(0)
  t:expect(r.stdout, "it says the anchor is malformed"):contains("malformed")
  t:expect(r.stdout, "and names where"):contains("oops.md")
  t:expect(r.stdout, "and why — the bad token"):contains("2026-13-45")
end)

prova.test("no [specs] section means the whole subsystem is inert", {
  proves = "the manifest entry IS the opt-in — a package that never asked for claims pays nothing and is never told about a subsystem it does not use",
}, function(t)
  local proj = t:use(sandbox)
  local bare = proj .. "/../bare"
  fs.mkdir(bare .. "/proofs")
  fs.write(bare .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(bare .. "/proofs/x_test.lua", 'prova.test("x", function(t) t:expect(1):equals(1) end)\n')

  local r = shell.run(prova.bin .. " owed", { cwd = bare, merge_stderr = true })

  -- The manifest entry IS the opt-in. A package that never asked for claims pays nothing and is
  -- never told about a subsystem it does not use.
  t:expect(r.code):equals(0)
  t:expect(r.stdout):never():contains("UNPROVEN")
end)

prova.test("a normal run ignores claims entirely", {
  proves = "prova must not parse markdown to run a test, and an unproven claim must never turn a green suite red",
}, function(t)
  local proj = t:use(sandbox)
  local r = shell.run(prova.bin, { cwd = proj, merge_stderr = true })

  -- Reconciliation belongs to the verb that asks for it. `prova` must not parse markdown to run a
  -- test, and an unproven claim must never turn a green suite red.
  t:expect(r.stdout):never():contains("UNPROVEN")
  t:expect(r.stdout):never():contains("DANGLING")
end)

prova.test("the deprecated `[specs] docs` list still scans, but warns", {
  proves = "existing projects on the flat `docs` list keep working — deprecation is a warning that points the way, not a silent break; `[[specs.source]]` is the one destination, and one way beats two spellings",
}, function(t)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n\n[specs]\ndocs = ["docs"]\n')
  fs.write(proj .. "/docs/design.md", "<!-- claim: still-scanned -->\nThe deprecated form still finds anchors.\n")
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- Backward compatible: the anchor is still found and owed.
  t:expect(r.stdout, "the deprecated form still scans"):contains("still-scanned")
  -- But it says so, and points at the one way.
  t:expect(r.stdout, "the deprecation is announced"):contains("deprecated")
  t:expect(r.stdout, "and points to `[[specs.source]]`"):contains("specs.source")
end)

-- ── Pinning: the claim's TEXT, not just its id ───────────────────────────────────────────────
--
-- The nastiest drift keeps everything green. An anchor survives, its prose is edited, the proof
-- still passes — and now discharges a claim it no longer matches. Nothing above catches that,
-- because every id still resolves.
--
-- A pin records the claim's text, so an edit is reported. Opt-in per binding: you pin the claims
-- that are load-bearing and leave the rest loose, because the churn is only worth it where the
-- exact wording is the contract.

local pinned = prova.fixture("claims-pin-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(proj .. "/docs/design.md", [[
<!-- claim: pinned-claim -->
Contention and absence are different answers.

<!-- claim: loose-claim -->
A held lease is never revoked.
]])
  return proj
end)

prova.test("`--pin` records the claim's text on the binding", {
  proves = "the pin has to be written by the tool: a hash a human types is a hash nobody verifies, and one that lands in the proof source is reviewable in a diff rather than hidden in a lockfile",
}, function(t)
  local proj = t:use(pinned)
  fs.write(proj .. "/proofs/pin_test.lua", [[
prova.test("covered", { covers = "docs/design.md#pinned-claim" }, function(t)
  t:expect(1):equals(1)
end)
]])
  local r = shell.run(prova.bin .. " owed --pin", { cwd = proj, merge_stderr = true })
  t:expect(r.code):equals(0)

  local source = fs.read(proj .. "/proofs/pin_test.lua")
  t:expect(source, "the binding gained a pin"):matches("docs/design.md#pinned%-claim@%x+")
end)

prova.test("editing a pinned claim is reported as STALE", {
  proves = "the drift that keeps everything green: the anchor resolves, the proof passes, and the claim now says something the proof does not check. Only the text can catch it",
}, function(t)
  local proj = t:use(pinned)
  fs.write(proj .. "/proofs/pin_test.lua", [[
prova.test("covered", { covers = "docs/design.md#pinned-claim" }, function(t)
  t:expect(1):equals(1)
end)
]])
  shell.run(prova.bin .. " owed --pin", { cwd = proj })

  fs.write(proj .. "/docs/design.md", [[
<!-- claim: pinned-claim -->
Contention and absence are different answers, and quota exhaustion is contention.

<!-- claim: loose-claim -->
A held lease is never revoked.
]])
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  t:expect(r.stdout, "the edit surfaces"):contains("STALE")
  t:expect(r.stdout):contains("pinned-claim")
end)

prova.test("an unpinned binding never goes stale", {
  proves = "opt-in per binding is what makes the churn acceptable: a claim whose exact wording is not the contract must not demand re-confirmation every time someone fixes a typo",
}, function(t)
  local proj = t:use(pinned)
  fs.write(proj .. "/proofs/loose_test.lua", [[
prova.test("covered loosely", { covers = "docs/design.md#loose-claim" }, function(t)
  t:expect(1):equals(1)
end)
]])
  fs.write(proj .. "/docs/design.md", [[
<!-- claim: pinned-claim -->
Contention and absence are different answers.

<!-- claim: loose-claim -->
A held lease is never revoked, ever, under any circumstances whatsoever.
]])
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/loose_test.lua")

  t:expect(r.stdout):never():contains("STALE")
end)

prova.test("whitespace-only edits do not churn a pin", {
  proves = "a pin that fired on reflowing a paragraph would be turned off within a week; normalising before hashing is what keeps it worth having",
}, function(t)
  local proj = t:use(pinned)
  fs.write(proj .. "/proofs/ws_test.lua", [[
prova.test("covered", { covers = "docs/design.md#pinned-claim" }, function(t)
  t:expect(1):equals(1)
end)
]])
  shell.run(prova.bin .. " owed --pin", { cwd = proj })

  -- Same words, reflowed and re-indented.
  fs.write(proj .. "/docs/design.md", [[
<!-- claim: pinned-claim -->
Contention   and absence
are different answers.

<!-- claim: loose-claim -->
A held lease is never revoked.
]])
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })
  fs.remove_all(proj .. "/proofs/ws_test.lua")

  t:expect(r.stdout):never():contains("STALE")
end)

prova.test("the binary teaches claims: catalog, topic, and the pin", {
  proves = "a capability an agent cannot discover does not exist. Discovery is two steps — the catalog must name it and the topic must explain it — and checking one leaves a capability either unfindable or unexplained",
}, function(t)
  local proj = t:use(pinned)

  local catalog = shell.run(prova.bin .. " learn", { cwd = proj, merge_stderr = true })
  t:expect(catalog.stdout, "the catalog names the topic"):contains("claims")

  local topic = shell.run(prova.bin .. " learn claims", { cwd = proj, merge_stderr = true })
  t:expect(topic.code):equals(0)
  t:expect(topic.stdout, "the anchor form"):contains("<!-- claim:")
  t:expect(topic.stdout, "the attribute"):contains("covers")
  t:expect(topic.stdout, "the verb"):contains("prova owed")
  t:expect(topic.stdout, "the opt-in"):contains("[specs]")
  t:expect(topic.stdout, "pinning"):contains("--pin")
end)

--- A package that uses its own local plugins — the shape every real project has, and the one the
--- obligation path did not resolve. `owed` and `attest` collect obligations through their own
--- slice of manifest resolution rather than the one a run uses; a `require("<local>")` in any proof
--- therefore failed to resolve and took the whole ledger down with it.
local with_plugin = prova.fixture("claims-plugin-sandbox", Scope.File, function(ctx)
  local proj = ctx:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.mkdir(proj .. "/.prova/plugins/helper")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\nplugin_root = ".prova/plugins"\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(proj .. "/.prova/plugins/helper/init.lua", "return { greet = function() return 1 end }\n")
  fs.write(proj .. "/docs/design.md",
    "# Design\n\n<!-- claim: helper-works -->\nThe helper answers.\n")
  fs.write(proj .. "/proofs/helper_test.lua", [[
local helper = require("helper")

prova.test("the helper answers", { covers = "docs/design.md#helper-works" }, function(t)
  t:expect(helper.greet()):equals(1)
end)
]])
  return proj
end)

prova.test("the ledger works in a package that uses its own plugins", {
  proves = "the obligation path must resolve a package exactly as a run does. Collecting obligations through a second, thinner slice of manifest resolution meant any proof that required a local plugin took `owed` and `attest` down with it — and every real project has local plugins",
}, function(t)
  local proj = t:use(with_plugin)
  local r = shell.run(prova.bin .. " owed", { cwd = proj, merge_stderr = true })

  -- Not a stack traceback, and not a false UNBOUND either: the binding is real and discharged.
  t:expect(r.stdout, "no Lua error escapes"):never():contains("stack traceback")
  t:expect(r.stdout):never():contains("no package root declared")
  t:expect(r.code):equals(0)
end)

prova.test("attest resolves a package that uses its own plugins", {
  proves = "attest reads bindings through the same collection path as owed, so the same defect silently made every attestation in a plugin-using package unanswerable",
}, function(t)
  local proj = t:use(with_plugin)
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " attest docs/design.md#helper-works",
    { cwd = proj, merge_stderr = true })

  t:expect(r.stdout):never():contains("stack traceback")
  t:expect(r.code, "the proof ran and passed, so the claim attests"):equals(0)
end)
