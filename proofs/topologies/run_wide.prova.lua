--- Run-wide topologies — `[topologies] … scope = "run"` provisions ONE instance for the whole run
--- and every declaring file binds it (docs/design/topologies.md#run-wide-topology-is-provisioned-once).
---
--- A `prova.topology(...)` in a proof file is a fixture, so a package whose proofs span N files
--- built the same environment N times in a cold run — three ten-container stacks and two kind
--- clusters, measured, to answer one question. This is the opt-in that says "once".
---
--- Resourceless on purpose: the factory bumps a count file (how many times it ran), stamps the
--- count into the value it returns (so two files can prove they saw the SAME instance), and appends
--- to a log in a `ctx:defer` (so the teardown's position relative to the tests is observable).
--- No Docker in the loop — this proves the sharing and the ownership, not what is shared.

local scratch = prova.fixture("run-wide-scratch", Scope.File, function(ctx)
  local nth = 0
  return function()
    nth = nth + 1
    return ctx:tempdir(tostring(nth))
  end
end)

local function run(dir, args, env)
  return shell.run(prova.bin .. " " .. args, { cwd = dir, env = env or {}, merge_stderr = true })
end

--- A package with TWO proof files declaring the same registered topology — the shape that paid N×.
--- `scope` is spliced into the `[topologies]` entry verbatim, so the same builder serves the
--- default (nil), the opt-in ("run"), and a value prova cannot honor.
local function package_with(root, scope)
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/plugins")
  local scope_key = scope and (", scope = %q"):format(scope) or ""
  fs.write(root .. "/prova.toml", ([[
[run]
proofs = ["proofs"]

[dependencies]
kitchen = "plugins/kitchen.lua"

[topologies]
orders = { package = "kitchen", factory = "orders"%s }
]]):format(scope_key))

  -- The factory: count its own runs, name the instance after that count, and log its teardown.
  fs.write(root .. "/plugins/kitchen.lua", [[
local M = {}
local function append(path, text)
  local prev = fs.exists(path) and fs.read(path) or ""
  fs.write(path, prev .. text .. ",")
end
function M.orders(ctx)
  local n = 0
  local count = os.getenv("PROVA_PROOF_COUNT")
  if count then
    n = (fs.exists(count) and tonumber(fs.read(count)) or 0) + 1
    fs.write(count, tostring(n))
  end
  local log = os.getenv("PROVA_PROOF_LOG")
  if log then ctx:defer(function() append(log, "torn-down") end) end
  -- Long enough that a second worker asking concurrently must WAIT rather than race past.
  if os.getenv("PROVA_PROOF_SLOW") then shell.run("sleep 1") end
  if os.getenv("PROVA_PROOF_FAIL") then error("the stack refused to come up") end
  return { svc = { url = "http://127.0.0.1:19999", instance = tostring(n) } }
end
return M
]])

  -- Two files, two suites, two Lua states: the duplication this exists to remove. Each records
  -- WHICH instance it saw — in its OWN file, so a parallel run cannot lose one observation to the
  -- other's read-modify-write — and also appends to the shared log, which is what makes the
  -- teardown's POSITION observable in a sequential run.
  for _, name in ipairs({ "one", "two" }) do
    fs.write(root .. "/proofs/" .. name .. "_test.lua", ([[
local env = prova.topology("orders", require("kitchen").orders)
prova.test("%s sees the environment", function(t)
  local e = t:use(env)
  local obs = os.getenv("PROVA_PROOF_OBS")
  if obs then fs.write(obs .. "/saw-%s.txt", e.svc.instance) end
  local log = os.getenv("PROVA_PROOF_LOG")
  if log then
    local prev = fs.exists(log) and fs.read(log) or ""
    fs.write(log, prev .. "%s:" .. e.svc.instance .. ",")
  end
  t:expect(e.svc.url):equals("http://127.0.0.1:19999")
end)
]]):format(name, name, name))
  end
end

--- What each file recorded about the instance it bound (nil if it never ran).
local function saw(root, name)
  local path = root .. "/saw-" .. name .. ".txt"
  return fs.exists(path) and fs.read(path) or nil
end

--- A third file that uses nothing — the target for a selection that must provision NOTHING.
local function add_unrelated_file(root)
  fs.write(root .. "/proofs/unrelated_test.lua", [[
prova.test("needs no environment", function(t)
  t:expect(1):equals(1)
end)
]])
end

prova.test("a run-wide topology is provisioned once and shared by every declaring file", {
  covers = {
    "docs/design/topologies.md#run-wide-topology-is-provisioned-once",
    -- The ergonomics finding this discharges: N declaring files, N worlds, measured at 33
    -- container creations for an eleven-container stack.
    "docs/design/agent-ergonomics.md#topology-fixture-is-file-local",
  },
}, function(t)
  local root = t:use(scratch)()
  package_with(root, "run")
  local count, log = root .. "/count.txt", root .. "/log.txt"

  local out = run(root, "", {
    PROVA_PROOF_COUNT = count,
    PROVA_PROOF_LOG = log,
    PROVA_PROOF_OBS = root,
  })
  t:expect(out.code, "both files pass"):equals(0)
  t:expect(fs.read(count), "the factory ran ONCE for the whole run"):equals("1")

  -- Identity, not just arithmetic: both files bound instance #1, so they shared one environment.
  t:expect(saw(root, "one"), "the first file bound instance 1"):equals("1")
  t:expect(saw(root, "two"), "and so did the second"):equals("1")

  -- The run owns it, and reaps it LAST: teardown lands after every suite, which is what makes the
  -- sharing safe rather than a leak.
  t:expect(fs.read(log), "torn down after the last file, by the run"):matches("torn%-down,$")

  -- Provisioned by this run, so the evidence stays hermetic: no live-state provenance, and nothing
  -- left behind for `prova ps` to report as a hold.
  local record = json.decode(fs.read(root .. "/.prova/var/last-run.json"))
  t:expect(#(record.attached or {}), "a run's own instance is not an attachment"):equals(0)
  t:expect(fs.exists(root .. "/.prova/var/running/orders.json"), "not a detached hold"):equals(false)
end)

prova.test("without the opt-in, a topology stays file-local — one instance per declaring file",
  { covers = "docs/design/topologies.md#file-local-is-still-the-default" }, function(t)
  local root = t:use(scratch)()
  package_with(root, nil)
  local count, log = root .. "/count.txt", root .. "/log.txt"

  local out = run(root, "", {
    PROVA_PROOF_COUNT = count,
    PROVA_PROOF_LOG = log,
    PROVA_PROOF_OBS = root,
  })
  t:expect(out.code):equals(0)
  t:expect(fs.read(count), "each file built its own"):equals("2")
  t:expect(saw(root, "one"), "the first file's own instance"):equals("1")
  t:expect(saw(root, "two"), "and a second one for the second file"):equals("2")
end)

prova.test("a run-wide topology is still demand-driven — a selection that never uses it pays nothing",
  { covers = "docs/design/topologies.md#run-wide-is-still-demand-driven" }, function(t)
  local root = t:use(scratch)()
  package_with(root, "run")
  add_unrelated_file(root)
  local count = root .. "/count.txt"

  local out = run(root, '-k "needs no environment"', { PROVA_PROOF_COUNT = count })
  t:expect(out.code):equals(0)
  t:expect(fs.exists(count), "nothing asked, so nothing was provisioned"):equals(false)
end)

prova.test("a sharing scope prova cannot honor is refused, never dropped",
  { covers = "docs/design/topologies.md#unknown-sharing-scope-is-refused" }, function(t)
  local root = t:use(scratch)()
  package_with(root, "session")
  local count = root .. "/count.txt"

  local out = run(root, "", { PROVA_PROOF_COUNT = count })
  t:expect(out.code, "refused, not run at file scope"):equals(2)
  t:expect(out.stdout, "names the key and both honorable values"):contains("scope")
  t:expect(out.stdout):contains('"run"')
  t:expect(fs.exists(count), "nothing was provisioned"):equals(false)
end)

prova.test("two workers asking at once still get one instance — the second waits, it never races",
  { covers = "docs/design/topologies.md#run-wide-provisioning-is-single-flight" }, function(t)
  local root = t:use(scratch)()
  package_with(root, "run")
  local count = root .. "/count.txt"

  -- Two suites on two workers, each with its own Lua state, both demanding the topology while the
  -- provision is still in flight: the shape a slot has to arbitrate. Each records its observation
  -- in its own file — a shared log would lose one of two concurrent read-modify-writes, and that
  -- flakiness would be the test's, not the runner's.
  local out = run(root, "-j 2", {
    PROVA_PROOF_COUNT = count,
    PROVA_PROOF_OBS = root,
    PROVA_PROOF_SLOW = "1",
  })
  t:expect(out.code):equals(0)
  t:expect(fs.read(count), "one provision, not two"):equals("1")
  t:expect(saw(root, "one"), "the claimer bound instance 1"):equals("1")
  t:expect(saw(root, "two"), "and the waiter bound the same one"):equals("1")
end)

prova.test("a run-wide provisioning that fails is memoized run-wide, not retried per file",
  { covers = "docs/design/topologies.md#run-wide-failure-is-memoized" }, function(t)
  local root = t:use(scratch)()
  package_with(root, "run")
  local count = root .. "/count.txt"

  local out = run(root, "", { PROVA_PROOF_COUNT = count, PROVA_PROOF_FAIL = "1" })
  t:expect(out.code, "the files that need it fail"):equals(1)
  t:expect(fs.read(count), "one attempt for the whole run"):equals("1")
  t:expect(out.stdout, "the first file reports what happened"):contains("refused to come up")
  t:expect(out.stdout, "and the second replays it as a memoized verdict"):contains("memoized")
end)

prova.test("a live holder still wins — attach outranks the run's own instance",
  { covers = "docs/design/topologies.md#attach-outranks-interning" }, function(t)
  local root = t:use(scratch)()
  package_with(root, "run")
  local count, log = root .. "/count.txt", root .. "/log.txt"

  local started = run(root, "start orders", { PROVA_PROOF_COUNT = count, PROVA_PROOF_LOG = log })
  t:defer(function() run(root, "down orders") end)
  t:expect(started.code):equals(0)
  t:expect(fs.read(count), "the holder provisioned once"):equals("1")

  local attached = run(root, "", { PROVA_PROOF_COUNT = count, PROVA_PROOF_LOG = log })
  t:expect(attached.code):equals(0)
  t:expect(fs.read(count), "the run provisioned nothing of its own"):equals("1")
  t:expect(attached.stdout, "and says it is testing live state"):contains("LIVE state")
  -- The holder is still the reaper: a run-wide pool never reaps what it did not provision.
  t:expect(fs.read(log), "no teardown fired"):never():contains("torn-down")
end)

prova.test("--fresh over a live holder is announced, because a fixed-name definition collides",
  { covers = "docs/design/topologies.md#fresh-over-a-holder-is-announced" }, function(t)
  local root = t:use(scratch)()
  package_with(root, "run")
  local count = root .. "/count.txt"

  run(root, "start orders", { PROVA_PROOF_COUNT = count })
  t:defer(function() run(root, "down orders") end)

  local fresh = run(root, "--fresh", { PROVA_PROOF_COUNT = count })
  t:expect(fresh.code):equals(0)
  t:expect(fresh.stdout, "the hazard is named, not discovered later"):contains("--fresh with topology")
  t:expect(fresh.stdout, "and both exits are spelled out"):contains("prova down orders")
  t:expect(fs.read(count), "--fresh provisioned its own instance"):equals("2")

  -- With nothing held, the same invocation says nothing — the warning is about the holder.
  run(root, "down orders")
  local quiet = run(root, "--fresh", { PROVA_PROOF_COUNT = count })
  t:expect(quiet.stdout):never():contains("--fresh with topology")
end)
