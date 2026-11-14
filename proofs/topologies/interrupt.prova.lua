--- An interrupted inhabited verb takes its environment with it
--- (docs/design/agent-ergonomics.md#interrupt-leaves-nothing-behind).
---
--- The shape of it. `prova start`'s child is deliberately in its own process group and
--- deliberately unleased — that is what "detached" means, and it is why `prova down` is its reaper
--- (verifiers.md#detached-topologies-hold-no-lease). Both facts are right, and together they meant
--- nothing at all reaped a holder whose supervisor was interrupted: `start` died of the Ctrl-C, the
--- `prova up` it spawned kept provisioning, and containers arrived with nobody left to report them.
--- Before registration there is not even a run-state record, so `prova down` answered "not running"
--- while the stack came up anyway. (`prova watch` had the same hole one layer down and was removed
--- rather than fixed — it had never re-applied, and nobody had ever used it.)
---
--- The marker is a FILE the factory writes and its teardown removes — deliberately not a spawned
--- process, which the conduct lease sweeps even on a death that runs no teardown
--- (verifiers.md#conduct-lease-survives-prova-death). A container is not in the holder's process
--- group either, so only the teardown actually running can release it: the file is the honest proxy,
--- and with a process marker these proofs pass against a kill that leaves every container behind.
---
--- Unix-gated, matching the claim: the graceful stop is a SIGTERM the holder handles, and Windows
--- has no signal that makes a detached holder run its teardown.

--- A package whose factory creates a "resource" (a file), defers releasing it, then hangs well past
--- any interrupt we will send — so every interrupt below lands mid-provision, which is the case
--- that used to orphan. `settle` makes the RELEASE slow too, for the case where a second interrupt
--- arrives while the holder is still working.
local function hanging_package(root, name, settle)
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/plugins")
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the suite runs", function(t) t:expect(1):equals(1) end)\n')
  fs.write(root .. "/plugins/slow.lua", [[
local M = {}
function M.slow(ctx)
  fs.write("]] .. root .. [[/created.txt", "the resource exists\n")
  ctx:defer(function()
    fs.write("]] .. root .. [[/releasing.txt", "the release began\n")
    shell.run("sleep ]] .. (settle or 0) .. [[")
    fs.write("]] .. root .. [[/released.txt", "torn down\n")
  end)
  shell.run("sleep 30")            -- never reaches registration on its own
  return { svc = { url = "http://127.0.0.1:19996" } }
end
return M
]])
  fs.write(root .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[dependencies]', 'slow = "plugins/slow.lua"', '',
    '[topologies]', name .. ' = { package = "slow", factory = "slow" }',
  }, "\n"))
end

--- Topology names of this run's own (`strays.name`): the holder's argv carries it, so the probe and
--- the reap see this run's holder, never another prova's running the same proof on the host (prova red
--- at 0325818f and 0f2d1687).
local strays = require("strays")

local function wait_until(cond, seconds)
  for _ = 1, seconds * 10 do
    if cond() then return true end
    shell.run("sleep 0.1")
  end
  return cond()
end

--- Is this run's holder for `name` still running?
local function holder_alive(name)
  return strays.alive("prova up", name)
end

prova.test("Ctrl-C on `prova start` stops the holder instead of orphaning it", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#interrupt-leaves-nothing-behind",
  proves = "the supervisor is the ONLY thing that can hear the interrupt — the holder sits in its own process group precisely so the terminal cannot reach it — so a `start` that simply died left a provisioning holder with no record, no supervisor and no reaper: `prova down` said `not running` while the containers kept arriving, and the operator's only remaining tool was `docker ps`",
}, function(t)
  local root = t:tempdir("start") .. "/pkg"
  local name = strays.name("interrupted")
  hanging_package(root, name)

  local proc = shell.spawn({ prova.bin, "start", name }, { cwd = root })
  t:defer(function()
    shell.run({ prova.bin, "down", name }, { cwd = root, merge_stderr = true, timeout = "60s" })
    strays.reap("prova up", name)
  end)

  t:expect(wait_until(function() return fs.exists(root .. "/created.txt") end, 60),
    "the factory got far enough to create the resource"):is_true()
  shell.run("kill -INT " .. proc.pid)

  t:expect(proc:wait(), "start reports the interrupt rather than a success it did not have")
    :equals(130)
  t:expect(proc:output(), "…and says what it did about it"):contains("interrupted")
  t:expect(wait_until(function() return fs.exists(root .. "/released.txt") end, 60),
    "the holder ran its teardown, so what it created was released"):is_true()
  t:expect(holder_alive(name), "no holder outlived the interrupted start"):is_false()

  -- And nothing is left claiming to be up: a stale record is its own kind of leak, because the
  -- next `start` refuses on it.
  local ps = shell.run({ prova.bin, "ps" }, { cwd = root, merge_stderr = true, timeout = "30s" })
  t:expect(ps.stdout, "no record survives the interrupt"):contains("no topologies running")
end)

prova.test("a second Ctrl-C stops the WAITING, never the holder mid-release", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#interrupt-leaves-nothing-behind",
  proves = "the tempting reading of an impatient second interrupt is `kill it already`, and obeying that is how containers get stranded: the holder is at that moment RUNNING the teardown that releases them, so killing it is the one action guaranteed to leave the mess behind. Stepping back instead costs the user nothing — the release finishes on its own — and the line must name the pid and the log rather than `prova ps`/`prova down`, which have no record to find this early",
}, function(t)
  local root = t:tempdir("second") .. "/pkg"
  local name = strays.name("lingering")
  hanging_package(root, name, 6)

  local proc = shell.spawn({ prova.bin, "start", name }, { cwd = root })
  t:defer(function()
    strays.reap("prova up", name)
  end)

  t:expect(wait_until(function() return fs.exists(root .. "/created.txt") end, 60),
    "the factory created its resource"):is_true()
  shell.run("kill -INT " .. proc.pid)
  -- The second interrupt must land while the holder is RELEASING: wait for the release to say it
  -- began, never a timed guess (a loaded host took longer than the 1 s this once slept).
  t:expect(wait_until(function() return fs.exists(root .. "/releasing.txt") end, 60),
    "the holder began its slow release"):is_true()
  shell.run("kill -INT " .. proc.pid)

  t:expect(proc:wait(), "the second interrupt ends the WAIT"):equals(130)
  t:expect(proc:output(), "…saying the holder was left to finish, not killed")
    :contains("still releasing")
  -- The proof that stepping back was right: nothing intervened, and the release completed anyway.
  t:expect(wait_until(function() return fs.exists(root .. "/released.txt") end, 60),
    "the holder finished releasing on its own"):is_true()
  t:expect(holder_alive(name), "…and then exited"):is_false()
end)
