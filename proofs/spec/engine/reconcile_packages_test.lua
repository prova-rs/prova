--- Reconciliation resolves the run's package set
--- (docs/design/agent-ergonomics.md#reminder-reconcile-ignores-adhoc-packages).
---
--- The attention account's `owed` comes from a pass that RE-EXECUTES the proof files to collect
--- their `covers`. Re-execution is the hazard: unless that pass resolves packages exactly as the
--- run did, one invocation gives two answers — the proofs pass against the `-P` package while the
--- pass dies on a function only that package has, leaving a green run with a silently stale
--- account. `-P` pointing at a working copy is the normal way to drive a package under edit, so
--- this is the everyday case, not an exotic one.

local scaffold = require("scaffold")

--- A package whose proof needs a function that exists ONLY in the ad-hoc copy: the declared
--- dependency resolves to a stub without it, so any pass that ignores `-P` fails loudly.
--- Returns the package root and the ad-hoc source path.
local function layered_package(t)
  local root = t:tempdir()
  fs.mkdir(root .. "/adhoc")
  fs.write(root .. "/adhoc/helper.lua",
    'return { adhoc_only = function() return "from the ad-hoc copy" end }\n')

  local proj = scaffold.package(t, {
    -- `[==[ … ]==]`: the TOML's own `[[specs.source]]` would close a plain `[[ … ]]` literal.
    manifest = [==[
[run]
proofs = ["proofs"]

[dependencies]
helper = "declared/helper.lua"

[[specs.source]]
type = "directory"
path = "docs"
]==],
    docs = { ["PLAN.md"] = "# Plan\n\n<!-- claim: the-account-has-something-to-count -->\nProse a proof binds.\n" },
    proofs = {
      ["covered_test.lua"] = [[
local helper = require("helper")

prova.test("uses the ad-hoc package's function", {
  covers = "docs/PLAN.md#the-account-has-something-to-count",
}, function(t)
  t:expect(helper.adhoc_only()):contains("ad-hoc")
end)
]],
      -- The reminder is the account's own witness: it can only report `owed == 0` if the pass
      -- executed the proof file (through the ad-hoc package) and found the claim covered.
      ["reminders.prova.lua"] = [[
prova.remind("ledger-was-read", {
  when = function(account)
    return account.owed == 0 and "the ledger reconciled: nothing owed" or false
  end,
}, "no action — this reminder reports that the account pass reached the ledger")
]],
    },
  })
  -- The manifest's declared copy: deliberately missing `adhoc_only`.
  fs.mkdir(proj .. "/declared")
  fs.write(proj .. "/declared/helper.lua", "return {}\n")
  return proj, root .. "/adhoc/helper.lua"
end

prova.test("a `-P`-layered run reconciles its account against the same package set", {
  covers = "docs/design/agent-ergonomics.md#reminder-reconcile-ignores-adhoc-packages",
  proves = "the observed shape: proofs green, then 'could not reconcile the ledger: attempt to call a nil value', because the pass resolved the manifest's declared source instead of the run's. A green run with a stale account is the dangerous outcome — nothing in the report says the ledger went unread",
}, function(t)
  local proj, adhoc = layered_package(t)
  local r = shell.run({ prova.bin, "-P", "helper=" .. adhoc },
    { cwd = proj, merge_stderr = true, timeout = "120s" })

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the proof itself passed against the ad-hoc copy"):contains("ad-hoc")
  t:expect(r.stdout, "reconciliation did not fail"):never():contains("could not reconcile")
  -- The account's witness fired, which takes BOTH halves: the pass loaded the file (so it used
  -- the run's package set) and counted the covered claim (so it collected the same obligations).
  t:expect(r.stdout, "the account reached the ledger and read it correctly")
    :contains("the ledger reconciled: nothing owed")
end)

prova.test("the ad-hoc layering is what makes that package load at all — the control", {
  covers = "docs/design/agent-ergonomics.md#reminder-reconcile-ignores-adhoc-packages",
  proves = "without this control the proof above passes vacuously: if the declared stub could satisfy the proof file, then a reconcile pass ignoring `-P` would succeed too and prove nothing. The same package set matters only where the sets genuinely differ",
}, function(t)
  local proj = layered_package(t)
  local r = shell.run({ prova.bin }, { cwd = proj, merge_stderr = true, timeout = "120s" })
  t:expect(r.code, "unlayered, the declared stub cannot serve the proof"):never():equals(0)
end)
