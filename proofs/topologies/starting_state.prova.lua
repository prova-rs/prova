--- A topology that is COMING UP is visible, guarded, and not bindable
--- (docs/design/agent-ergonomics.md §38).
---
--- The field report (2026-08-25, ybor-studio): an agent's `prova start` was mid-`kind create` when
--- a human ran the same verb. The second start was not told "already starting" — it raced into the
--- factory and died on kind's own `node(s) already exist for a cluster with the name "…"`. In
--- between, `prova ps` said **no topologies running**, which was true by the letter (the record
--- landed only at ready) and useless in the moment.
---
--- The record now appears at BIRTH with `status: starting` and flips to `ready`. That single change
--- reaches four consumers, and three of them are proven here rather than the one that motivated it —
--- because the dangerous half of this change is not the guard, it is everything that reads a record
--- and would happily bind to one carrying nothing.

--- A package whose factory takes `hold` seconds before returning, so it can be observed mid-flight.
local function slow_package(root, name, hold)
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/plugins")
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the suite runs", function(t) t:expect(1):equals(1) end)\n')
  fs.write(root .. "/plugins/svc.lua", [[
local M = {}
function M.web(ctx)
  shell.run("sleep ]] .. hold .. [[")
  return { svc = { url = "http://127.0.0.1:44441" } }
end
return M
]])
  fs.write(root .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[dependencies]', 'svc = "plugins/svc.lua"', '',
    '[topologies]', name .. ' = { package = "svc", factory = "web" }',
  }, "\n"))
end

local function wait_until(cond, seconds)
  for _ = 1, seconds * 10 do
    if cond() then return true end
    shell.run("sleep 0.1")
  end
  return cond()
end

--- Hold `name` in its STARTING window and hand the body a live starting topology. Everything is
--- reaped whether the body passes or not.
local function while_starting(t, root, name, body)
  local proc = shell.spawn({ prova.bin, "start", name }, { cwd = root })
  t:defer(function()
    shell.run({ prova.bin, "down", name }, { cwd = root, merge_stderr = true, timeout = "60s" })
    proc:stop()
  end)
  local record = root .. "/.prova/var/running/" .. name .. ".json"
  t:expect(wait_until(function() return fs.exists(record) end, 60),
    "the holder registered before it finished provisioning"):is_true()
  body()
end

prova.test("`prova ps` names a topology that is still coming up, and a second start is refused", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#starting-is-a-visible-state",
  proves = "the record used to land only at ready, so the fifteen minutes a kind cluster spends coming up were indistinguishable from nothing running at all — `ps` said the machine was idle while it was very much occupied, and the one tool that answers `what is up?` had no idea",
}, function(t)
  local root = t:tempdir("visible") .. "/pkg"
  slow_package(root, "coming-up", 15)

  while_starting(t, root, "coming-up", function()
    local ps = shell.run({ prova.bin, "ps" }, { cwd = root, merge_stderr = true, timeout = "30s" })
    t:expect(ps.stdout, "ps names the topology at all"):contains("coming-up")
    t:expect(ps.stdout, "…and says it is STARTING, not running"):contains("starting")
    t:expect(ps.stdout, "…and does not claim it is up"):never():contains("no topologies running")
  end)
end)

prova.test("a second `start` will not race the first into the factory", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#second-start-joins-or-refuses",
  proves = "the collision the second invoker actually saw was kind's — `node(s) already exist for a cluster with the name` — which names the tool being driven and not the cause, three layers from the fact that someone else was already standing the same thing up",
}, function(t)
  local root = t:tempdir("refuse") .. "/pkg"
  slow_package(root, "contended", 15)

  while_starting(t, root, "contended", function()
    local second = shell.run({ prova.bin, "start", "contended" },
      { cwd = root, merge_stderr = true, timeout = "60s" })
    t:expect(second.code, "the second start refuses instead of provisioning"):never():equals(0)
    t:expect(second.stdout, "…naming the state it is in"):contains("already starting")
    t:expect(second.stdout, "…and what to do about it"):contains("prova down contended")
  end)
end)

prova.test("a run does not ATTACH to a topology that is still coming up", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#attach-must-not-bind-a-starting-topology",
  proves = "the trap inside the fix: attach gates on `is_alive` alone, and a mid-startup holder is alive. Its record deliberately carries no endpoints and a null value, so binding to it would hand `t:use(env)` a nil while announcing `attach to its LIVE state` — moving the damage from the inhabited verbs, where a collision is loud, to the run path, where it is silent, and firing for one person alone where the collision needed two",
}, function(t)
  local root = t:tempdir("attach") .. "/pkg"
  slow_package(root, "half-up", 15)

  while_starting(t, root, "half-up", function()
    local run = shell.run({ prova.bin }, { cwd = root, merge_stderr = true, timeout = "60s" })
    t:expect(run.stdout, "no run may bind to a topology that has not come up"):never()
      :contains("held topology")

    -- The same hole one line down: `--topology` must not count a starting holder as offered,
    -- or a strict requirement is satisfied by something carrying nothing.
    local strict = shell.run({ prova.bin, "--topology", "half-up" },
      { cwd = root, merge_stderr = true, timeout = "60s" })
    t:expect(strict.code, "a strict requirement is not met by a topology still coming up")
      :never():equals(0)
    t:expect(strict.stdout, "…and says so in the requirement's own words")
      :contains("no held topology by that name is running")
  end)
end)

prova.test("a record written before `status` existed still parses, and reads as ready", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#run-state-is-a-versioned-contract",
  proves = "the upgrade case nobody runs: a holder started on the previous release has a record with no `status` field. Without a default it fails to deserialize, `read` returns nil, and that live holder goes invisible to ps/down/the guard — the exact defect this field was added to fix, reintroduced by its own fix, for precisely the people who had something held while upgrading",
}, function(t)
  local root = t:tempdir("migrate") .. "/pkg"
  slow_package(root, "legacy", 1)
  fs.mkdir(root .. "/.prova/var/running")

  -- A live process to own the record, so `ps` cannot dismiss it as stale.
  local holder = shell.spawn("sleep 30")
  t:defer(function() holder:stop() end)

  -- Exactly the shape the previous release wrote: no `status` key at all.
  fs.write(root .. "/.prova/var/running/legacy.json", json.encode({
    name = "legacy",
    pid = holder.pid,
    started_at = 1,
    endpoints = { { name = "svc", url = "http://127.0.0.1:44442" } },
    value = { svc = { url = "http://127.0.0.1:44442" } },
  }))

  local ps = shell.run({ prova.bin, "ps" }, { cwd = root, merge_stderr = true, timeout = "30s" })
  t:expect(ps.stdout, "the pre-status record is still readable"):contains("legacy")
  t:expect(ps.stdout, "…and an absent status means it came up"):contains("running")
  t:expect(ps.stdout, "…so its endpoints survive the upgrade"):contains("44442")
end)

prova.test("a stale STARTING record warns that resources may be lying around", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#a-stale-starting-record-implies-residue",
  proves = "the case that recurs: the live race needs two actors at once, this one needs one crash. A holder killed mid-factory leaves the record AND a half-built cluster, and clearing the record removes the evidence without removing the cluster — so the next attempt fails on a name already taken and the operator goes off to diagnose a conflict that is really the previous failure's residue",
}, function(t)
  local root = t:tempdir("residue") .. "/pkg"
  slow_package(root, "crashed", 1)
  fs.mkdir(root .. "/.prova/var/running")

  -- A STARTING record whose holder is gone: the shape a killed factory leaves behind. Written as
  -- literal JSON rather than through `json.encode`, which renders an empty Lua table as `{}` — an
  -- object where `endpoints` must be an array, so the record would not parse and would be read as
  -- ABSENT, quietly turning this proof into a test of nothing.
  fs.write(root .. "/.prova/var/running/crashed.json",
    '{"name":"crashed","pid":999999999,"started_at":1,"status":"starting",'
    .. '"endpoints":[],"value":null}')

  local r = shell.run({ prova.bin, "start", "crashed" },
    { cwd = root, merge_stderr = true, timeout = "120s" })
  t:defer(function()
    shell.run({ prova.bin, "down", "crashed" }, { cwd = root, merge_stderr = true, timeout = "60s" })
  end)

  -- It proceeds — prova cannot know what a factory created — but it says what it is stepping over.
  t:expect(r.stdout, "the stale STARTING record is called out, not silently deleted")
    :contains("stale STARTING record")
  t:expect(r.stdout, "…and names the consequence in the words the next failure will wear")
    :contains("residue")
  t:expect(r.code, "…while still letting this attempt proceed"):equals(0)
end)

-- ── run-state integrity: the record is a contract, not a hint ────────────────────────────────

prova.test("a record that cannot be parsed is refused, not read as nothing held", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#unparseable-runstate-record-reads-as-no-hold",
  proves = "the fail-open direction: `read` was `from_str(...).ok()` and `list` dropped anything that would not deserialize, so a corrupt record made a LIVE holder invisible and the guards provisioned a second instance over it. Found twice — the second time while proving the stale-record guard, where a fixture wrote `endpoints` as `{}` and the proof passed for the wrong reason because nothing anywhere said the file was unreadable",
}, function(t)
  local root = t:tempdir("corrupt") .. "/pkg"
  slow_package(root, "garbled", 1)
  fs.mkdir(root .. "/.prova/var/running")
  -- Exactly the shape that fooled the earlier proof: `endpoints` as an object, not an array.
  fs.write(root .. "/.prova/var/running/garbled.json",
    '{"name":"garbled","pid":1,"started_at":1,"endpoints":{},"value":{}}')

  local up = shell.run({ prova.bin, "start", "garbled" },
    { cwd = root, merge_stderr = true, timeout = "60s" })
  t:expect(up.code, "an unreadable record must not read as a free name"):never():equals(0)
  t:expect(up.stdout, "…the refusal says what it could not do"):contains("cannot be read")
  t:expect(up.stdout, "…and names the file, since looking at it is the only way out")
    :contains("garbled.json")

  -- `ps` must not stay quiet about it either: it is the tool people ask "what is up?"
  local ps = shell.run({ prova.bin, "ps" }, { cwd = root, merge_stderr = true, timeout = "30s" })
  t:expect(ps.stdout, "ps reports the entry it cannot read"):contains("unreadable")
  t:expect(ps.stdout, "…by name"):contains("garbled")
  t:expect(ps.stdout, "…rather than claiming the machine is idle")
    :never():contains("no topologies running")
end)

prova.test("concurrent starts of one name reach the factory exactly once", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#claiming-a-topology-name-is-atomic",
  proves = "a race that is merely rare is indistinguishable from one that is fixed, right up until it costs a cluster. Read-then-write left both of two simultaneous starts seeing nothing and proceeding; only an atomic claim makes `exactly one` true rather than likely",
}, function(t)
  local root = t:tempdir("race") .. "/pkg"
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/plugins")
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the suite runs", function(t) t:expect(1):equals(1) end)\n')
  -- The factory appends a line per invocation: the count IS the assertion.
  fs.write(root .. "/plugins/svc.lua", [[
local M = {}
function M.web(ctx)
  local f = io.open("]] .. root .. [[/factory.log", "a")
  f:write("ran\n"); f:close()
  shell.run("sleep 6")
  return { svc = { url = "http://127.0.0.1:44444" } }
end
return M
]])
  fs.write(root .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[dependencies]', 'svc = "plugins/svc.lua"', '',
    '[topologies]', 'raced = { package = "svc", factory = "web" }',
  }, "\n"))

  t:defer(function()
    shell.run({ prova.bin, "down", "raced" }, { cwd = root, merge_stderr = true, timeout = "60s" })
    shell.run("pkill -f 'prova up race[d]' 2>/dev/null; true")
  end)

  -- Fire them together and let them fight over the name.
  local procs = {}
  for _ = 1, 6 do
    table.insert(procs, shell.spawn({ prova.bin, "up", "raced" }, { cwd = root }))
  end
  shell.run("sleep 4")

  local ran = 0
  for _ in (fs.exists(root .. "/factory.log") and fs.read(root .. "/factory.log") or ""):gmatch("ran") do
    ran = ran + 1
  end
  for _, p in ipairs(procs) do p:stop() end

  t:expect(ran, "exactly one of six concurrent starts reached the factory (got " .. ran .. ")")
    :equals(1)
end)
