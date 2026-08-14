--- Black-box surface of the inhabited verbs — the registration door, the running record, and
--- the detached supervisor that reuses the one in-process teardown path.
---
--- The contract (docs/design/topologies.md): `[topologies]` is the whole surface for
--- `up`/`start`/`watch`/`ps` and no proof files are loaded; a topology declared only in a test
--- file is refused with the entry to add; a running `up` self-registers a record (pid +
--- endpoints) under `<home>/.prova/var/running/` and removes it on clean teardown; `start` is a
--- supervisor over attached `up`, so `down` runs the SAME `ctx:manage`/`ctx:defer` teardown in
--- the detached child.
---
--- Everything here is deliberately resourceless — a factory returning plain `{ url = ... }`
--- shapes — so the verb machinery is pinned without Docker in the loop.

local scratch = prova.fixture("topology-verbs-scratch", Scope.File, function(ctx)
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

--- A package with one registered, resourceless topology and one green proof. The factory defers
--- a marker write (path via PROVA_PROOF_MARKER) so its teardown is observable from outside the
--- process that runs it.
local function registered(root)
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
  local marker = os.getenv("PROVA_PROOF_MARKER")
  if marker then ctx:defer(function() fs.write(marker, "torn-down") end) end
  return { svc = { url = "http://127.0.0.1:19999" } }
end
return M
]])
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the suite runs", function(t) t:expect(1):equals(1) end)\n')
end

-- ── the registration door ────────────────────────────────────────────────────────────────────

prova.test("the inhabited verbs read [topologies] and load no proof files",
  { covers = "docs/design/topologies.md#registration-is-the-only-door" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  -- A proof file that raises at load. If an inhabited verb loaded proof files, this would kill
  -- it — the run path proves the mine is armed, the verb path proves it was never stepped on.
  fs.write(root .. "/proofs/broken_test.lua",
    'error("this file must never load under an inhabited verb")\n')

  local r = run(root, "start orders")
  t:defer(function() run(root, "down orders") end)
  t:expect(r.code, "stands up despite the broken proof file"):equals(0)
  t:expect(r.stdout, "endpoints printed"):contains("http://127.0.0.1:19999")
  t:expect(r.stdout):never():contains("must never load")

  local test_path = run(root, "")
  t:expect(test_path.code, "the run path DOES load it — the mine is real"):never():equals(0)
end)

prova.test("a topology declared only in a test file is refused, and the refusal teaches the fix",
  { covers = "docs/design/topologies.md#test-only-topology-is-not-addressable" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/topo_test.lua", [[
local env = prova.topology("hidden", function(ctx) return { svc = { url = "http://x" } } end)
prova.test("uses it", function(t) t:use(env) t:expect(1):equals(1) end)
]])

  local r = run(root, "up hidden")
  t:expect(r.code):equals(2)
  t:expect(r.stdout, "names the missing topology"):contains('no topology "hidden"')
  t:expect(r.stdout, "prints the entry to add"):contains("[topologies]")
  t:expect(r.stdout):never():contains("no topologies defined")

  -- The same declaration is untouched as a fixture: the test door still works.
  local test_path = run(root, "")
  t:expect(test_path.code, "the test path still consumes it"):equals(0)
end)

-- ── the running record ───────────────────────────────────────────────────────────────────────

prova.test("a running topology self-registers pid and endpoints, and clean teardown removes the record",
  { covers = "docs/design/topologies.md#up-self-registers" }, function(t)
  local root = t:use(scratch)()
  registered(root)

  run(root, "start orders")
  t:defer(function() run(root, "down orders") end)

  local record_path = root .. "/.prova/var/running/orders.json"
  t:expect(record_path):exists()
  local record = json.decode(fs.read(record_path))
  t:expect(record.pid, "the pid to signal"):is_truthy()
  t:expect(record.endpoints[1].url, "the endpoints as connect strings")
    :equals("http://127.0.0.1:19999")

  run(root, "down orders")
  t:expect(fs.exists(record_path), "clean teardown removes the record"):equals(false)
end)

-- ── the detached supervisor ──────────────────────────────────────────────────────────────────

prova.test("start leaves it running, ps sees it, and down runs the same teardown in the child",
  { covers = "docs/design/topologies.md#detached-supervises-attached" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local marker = root .. "/marker.txt"

  local started = run(root, "start orders", { PROVA_PROOF_MARKER = marker })
  t:defer(function() run(root, "down orders") end)
  t:expect(started.code):equals(0)
  t:expect(fs.exists(marker), "still running — teardown has not fired"):equals(false)

  local ps = run(root, "ps")
  t:expect(ps.stdout):contains("orders")
  t:expect(ps.stdout, "ps reports the endpoints too"):contains("http://127.0.0.1:19999")

  local down = run(root, "down orders")
  t:expect(down.code):equals(0)
  -- The teardown runs in the detached CHILD on SIGTERM, so give it a beat to land.
  for _ = 1, 20 do
    if fs.exists(marker) then break end
    shell.run("sleep 0.1")
  end
  t:expect(marker):exists()
  t:expect(fs.read(marker), "the factory's own deferred teardown ran"):equals("torn-down")
end)
