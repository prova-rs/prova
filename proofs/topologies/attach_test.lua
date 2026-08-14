--- Held-topology attach — a plain run binds to a running holder by name instead of provisioning
--- (docs/design/topologies.md "Held-topology attach"): the factory does not re-run, the holder
--- keeps teardown sovereignty, `--fresh` opts out, `--topology` insists, and the run record
--- carries live-state provenance.
---
--- Everything here is deliberately resourceless — the factory bumps a count file (how many times
--- it ran) and defers a marker write (whether teardown fired), so attach-vs-provision and
--- reap-vs-hold are observable across processes without Docker in the loop.

local scratch = prova.fixture("topology-attach-scratch", Scope.File, function(ctx)
  -- Each call names its own directory, so asking twice for "1" is the same place and
  -- the scratch tree on disk says which sandbox is which.
  local nth = 0
  return function()
    nth = nth + 1
    return ctx:tempdir(tostring(nth))
  end
end)

local function run(dir, args, env)
  return shell.run(prova.bin .. " " .. args, { cwd = dir, env = env or {}, merge_stderr = true })
end

--- A package whose topology is declared through BOTH doors — registered in [topologies] for the
--- holder, and declared in a proof file (the same factory) for the run path — the one-definition
--- shape attach exists to serve.
local function both_doors(root)
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/plugins")
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
kitchen = "plugins/kitchen.lua"

[topologies]
orders = { package = "kitchen", factory = "orders" }
]])
  fs.write(root .. "/plugins/kitchen.lua", [[
local M = {}
function M.orders(ctx)
  local count = os.getenv("PROVA_PROOF_COUNT")
  if count then
    local n = fs.exists(count) and tonumber(fs.read(count)) or 0
    fs.write(count, tostring(n + 1))
  end
  local marker = os.getenv("PROVA_PROOF_MARKER")
  if marker then ctx:defer(function() fs.write(marker, "torn-down") end) end
  return { svc = { url = "http://127.0.0.1:19999" } }
end
return M
]])
  fs.write(root .. "/proofs/topo_test.lua", [[
local env = prova.topology("orders", require("kitchen").orders)
prova.test("sees the environment", function(t)
  local e = t:use(env)
  t:expect(e.svc.url):equals("http://127.0.0.1:19999")
end)
]])
end

prova.test("a run binds to the held instance by name — the factory does not re-run",
  { covers = "docs/design/topologies.md#attach-binds-by-name" }, function(t)
  local root = t:use(scratch)()
  both_doors(root)
  local count = root .. "/count.txt"

  local started = run(root, "start orders", { PROVA_PROOF_COUNT = count })
  t:defer(function() run(root, "down orders") end)
  t:expect(started.code):equals(0)
  t:expect(fs.read(count), "the holder provisioned once"):equals("1")

  -- The holder's record carries the rehydration payload.
  local record = json.decode(fs.read(root .. "/.prova/var/running/orders.json"))
  t:expect(record.value.svc.url, "the recorded value snapshot"):equals("http://127.0.0.1:19999")

  local attached = run(root, "", { PROVA_PROOF_COUNT = count })
  t:expect(attached.code, "the suite passes against the held instance"):equals(0)
  t:expect(fs.read(count), "the factory did NOT run again"):equals("1")
  t:expect(attached.stdout, "attachment is announced, never silent"):contains("LIVE state")

  -- The mine is armed: with the holder gone, the same run provisions.
  run(root, "down orders")
  local cold = run(root, "", { PROVA_PROOF_COUNT = count })
  t:expect(cold.code):equals(0)
  t:expect(fs.read(count), "no holder → the factory provisions"):equals("2")
end)

prova.test("an attached run never tears the holder down",
  { covers = "docs/design/topologies.md#attach-leaves-holder-sovereign" }, function(t)
  local root = t:use(scratch)()
  both_doors(root)
  local marker = root .. "/marker.txt"

  run(root, "start orders", { PROVA_PROOF_MARKER = marker })
  t:defer(function() run(root, "down orders") end)

  local attached = run(root, "")
  t:expect(attached.code):equals(0)
  t:expect(fs.exists(marker), "the holder's teardown has not fired"):equals(false)
  t:expect(fs.exists(root .. "/.prova/var/running/orders.json"), "still held"):equals(true)

  run(root, "down orders")
  for _ = 1, 20 do
    if fs.exists(marker) then break end
    shell.run("sleep 0.1")
  end
  t:expect(fs.read(marker), "only down reaps — the same teardown as ever"):equals("torn-down")
end)

prova.test("--fresh ignores the holder and provisions",
  { covers = "docs/design/topologies.md#fresh-opts-out" }, function(t)
  local root = t:use(scratch)()
  both_doors(root)
  local count = root .. "/count.txt"

  run(root, "start orders", { PROVA_PROOF_COUNT = count })
  t:defer(function() run(root, "down orders") end)
  t:expect(fs.read(count)):equals("1")

  local fresh = run(root, "--fresh", { PROVA_PROOF_COUNT = count })
  t:expect(fresh.code):equals(0)
  t:expect(fs.read(count), "provisioned its own instance"):equals("2")
  t:expect(fresh.stdout):never():contains("LIVE state")
  t:expect(fs.exists(root .. "/.prova/var/running/orders.json"), "holder untouched"):equals(true)

  local contradiction = run(root, "--fresh --topology orders")
  t:expect(contradiction.code, "--fresh with --topology is refused"):equals(2)
end)

prova.test("--topology requires the attachment or fails loudly",
  { covers = "docs/design/topologies.md#require-topology-is-strict" }, function(t)
  local root = t:use(scratch)()
  both_doors(root)

  -- Nothing held: strict mode refuses rather than quietly provisioning fresh.
  local refused = run(root, "--topology orders")
  t:expect(refused.code):equals(2)
  t:expect(refused.stdout, "teaches the fix"):contains("prova start orders")

  -- Held: the same invocation attaches and passes — the refusal above was about the holder,
  -- not the flag.
  run(root, "start orders")
  t:defer(function() run(root, "down orders") end)
  local strict = run(root, "--topology orders")
  t:expect(strict.code):equals(0)
  t:expect(strict.stdout):contains("LIVE state")
end)

prova.test("the run record carries live-state provenance",
  { covers = "docs/design/topologies.md#attach-is-recorded" }, function(t)
  local root = t:use(scratch)()
  both_doors(root)

  run(root, "start orders")
  t:defer(function() run(root, "down orders") end)
  run(root, "")
  local record = json.decode(fs.read(root .. "/.prova/var/last-run.json"))
  t:expect(record.attached[1], "attached runs say so durably"):equals("orders")

  run(root, "--fresh")
  local hermetic = json.decode(fs.read(root .. "/.prova/var/last-run.json"))
  t:expect(#(hermetic.attached or {}), "hermetic runs carry no attachment"):equals(0)
end)
