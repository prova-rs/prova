-- `prova start`'s budget is declarable and its expiry is clean
-- (docs/design/agent-ergonomics.md#start-timeout-is-unconfigurable, #start-timeout-orphans-containers).
--
-- The field report: a Kubernetes topology that honestly needs five to eight minutes could never be
-- inhabited, because start's window was a fixed 300s with no flag and no manifest key — the same
-- factory a suite fixture builds happily. And when the window expired, start SIGKILLed the holder,
-- so the containers it had already created outlived it and the next attempt failed on a port
-- conflict that was really the previous failure's residue.
--
-- Proven in the short direction: a topology that declares a TINY budget must fail at that budget
-- rather than at 300s. That is the same code path a 15m declaration takes, and it is provable in
-- seconds rather than minutes.

--- A package whose topology factory never registers: it spawns a marker process, defers its
--- teardown, and then sleeps past any budget. `start` must give up, and the teardown must run.
local function hanging_package(t, token, decl)
  local dir = t:tempdir() .. "/pkg"
  fs.mkdir(dir .. "/proofs")
  fs.mkdir(dir .. "/plugins")
  fs.write(dir .. "/proofs/x_test.lua",
    'prova.test("t", function(t) t:expect(true):is_true() end)\n')
  fs.write(dir .. "/plugins/slow.lua", [[
local M = {}
function M.slow(ctx)
  -- The resource this topology "created": it must not outlive a start that gives up.
  local proc = shell.spawn("sleep ]] .. token .. [[")
  ctx:defer(function() proc:stop() end)
  shell.run("sleep 30")            -- never reaches registration
  return { svc = { url = "http://127.0.0.1:1" } }
end
return M
]])
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[dependencies]', 'slow = "plugins/slow.lua"', '',
    '[topologies]', 'hang = { package = "slow", factory = "slow"' .. decl .. ' }',
  }, "\n"))
  return dir
end

--- `77.9x` probes as `77[.]9x`: Linux pgrep -f matches the wrapping shell's own argv.
local function alive(token)
  return shell.run("pgrep -f 'sleep " .. (token:gsub("%.", "[.]")) .. "'").code == 0
end

local function reap(token)
  shell.run("pkill -f 'sleep " .. (token:gsub("%.", "[.]")) .. "' 2>/dev/null; true")
end

prova.test("a topology's declared `startup` is the budget start honors", {
  covers = "docs/design/agent-ergonomics.md#start-timeout-is-unconfigurable",
  proves = "the definition knows its own cost, and without a way to say so the inhabited half of the inhabited/fixture pair is simply unavailable to any stack slower than five minutes — a kind cluster with eight rollouts can be a suite fixture and can never be `prova start`ed",
}, function(t)
  local token = "77.911"
  local dir = hanging_package(t, token, ', startup = "2s"')

  local started = os.time()
  local r = shell.run({ prova.bin, "start", "hang" }, { cwd = dir, merge_stderr = true, timeout = "120s" })
  local elapsed = os.time() - started
  shell.run({ prova.bin, "down", "hang" }, { cwd = dir, merge_stderr = true, timeout = "60s" })
  reap(token)

  t:expect(r.code, "start gives up"):never():equals(0)
  t:expect(elapsed, "…at the DECLARED budget, not the 300s default: " .. elapsed .. "s"):lt(60)
  t:expect(r.stdout, "the message names the budget that fired"):contains("2s")
  t:expect(r.stdout, "…and how to change it in the definition"):contains("startup")
  t:expect(r.stdout, "…and for this invocation"):contains("--timeout")
end)

prova.test("`--timeout` overrides the declaration, and the declaration overrides the default", {
  covers = "docs/design/agent-ergonomics.md#start-timeout-is-unconfigurable",
  proves = "the machine having a bad day is not the definition being wrong: an override that required editing the manifest would make every slow CI runner a source-control event",
}, function(t)
  local token = "77.922"
  -- A generous declaration, overridden downward by the flag: only the flag can explain a fast exit.
  local dir = hanging_package(t, token, ', startup = "10m"')

  local started = os.time()
  local r = shell.run({ prova.bin, "start", "hang", "--timeout", "2s" },
    { cwd = dir, merge_stderr = true, timeout = "120s" })
  local elapsed = os.time() - started
  shell.run({ prova.bin, "down", "hang" }, { cwd = dir, merge_stderr = true, timeout = "60s" })
  reap(token)

  t:expect(r.code):never():equals(0)
  t:expect(elapsed, "the flag won over a 10m declaration: " .. elapsed .. "s"):lt(60)
  t:expect(r.stdout, "…and the message names the budget that actually fired"):contains("2s")
end)

prova.test("a start that gives up runs the holder's teardown", {
  covers = "docs/design/agent-ergonomics.md#start-timeout-orphans-containers",
  proves = "the residue defect: a SIGKILLed holder runs no teardown, so the CONTAINERS it created survive and the next attempt fails on a host port they still hold — reported as a port conflict, which sends the operator to diagnose networking instead of the previous failure. The cure was `docker ps -q | xargs docker rm -f`, which a user should never need to know",
}, function(t)
  -- The marker is a FILE the factory creates and its teardown removes — deliberately not a spawned
  -- process, which the conduct lease sweeps even on SIGKILL (verifiers.md#conduct-lease-survives-
  -- prova-death). A container is not in the holder's process group either, so only the teardown
  -- actually running can release it: the file is the honest proxy, and with a process marker this
  -- proof passed against a SIGKILL that leaves every container behind.
  local dir = t:tempdir() .. "/pkg"
  fs.mkdir(dir .. "/proofs")
  fs.mkdir(dir .. "/plugins")
  local marker = dir .. "/created.txt"
  fs.write(dir .. "/proofs/x_test.lua",
    'prova.test("t", function(t) t:expect(true):is_true() end)\n')
  local released = dir .. "/released.txt"
  fs.write(dir .. "/plugins/slow.lua", [[
local M = {}
function M.slow(ctx)
  fs.write("]] .. marker .. [[", "the resource exists\n")
  ctx:defer(function() fs.write("]] .. released .. [[", "torn down\n") end)
  shell.run("sleep 30")            -- never reaches registration
  return { svc = { url = "http://127.0.0.1:1" } }
end
return M
]])
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[dependencies]', 'slow = "plugins/slow.lua"', '',
    '[topologies]', 'hang = { package = "slow", factory = "slow", startup = "2s" }',
  }, "\n"))

  local r = shell.run({ prova.bin, "start", "hang" }, { cwd = dir, merge_stderr = true, timeout = "120s" })
  t:expect(r.code, "start gave up"):never():equals(0)
  -- Two facts, each a file, so neither depends on when the assertion runs relative to the teardown.
  t:expect(fs.exists(marker), "the factory got far enough to create the resource"):is_true()

  local torn = false
  for _ = 1, 100 do
    if fs.exists(released) then torn = true; break end
    shell.run("sleep 0.1")
  end
  t:expect(torn, "…and the holder's teardown ran, so what it created was released"):is_true()
end)
