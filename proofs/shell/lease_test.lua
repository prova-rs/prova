--- The conduct lease (docs/design/verifiers.md — three claims): a conduct dies as a TREE on
--- every controlled kill; prova's own death — however it dies — sweeps every leased group via
--- the reaper sidecar; and `prova start` provisions are deliberately unleased. Unique sleep
--- durations are the process tokens; every test reaps its strays whether it passes or not.
---
--- Everything here exercises the SUBJECT: conducts run inside `prova.bin` (spawned as the inner
--- prova, or reached via `prova.bin eval`); the outer kills use raw `kill`, never the
--- conductor's own process machinery, so an old conductor cannot fake or break a verdict.

local function alive(token)
  return shell.run("pgrep -f 'sleep " .. token .. "'").code == 0
end

local function wait_until(cond, seconds)
  for _ = 1, seconds * 10 do
    if cond() then return true end
    shell.run("sleep 0.1")
  end
  return cond()
end

local function reap_stray(token)
  shell.run("pkill -f 'sleep " .. token .. "' 2>/dev/null; true")
end

prova.test("a bound's kill reaps the whole tree — a backgrounded grandchild dies with the shell", {
  covers = "docs/design/verifiers.md#conduct-process-group-reaping",
  proves = "the direct-child kill left a pipeline's grandchildren holding exactly the locks the red report freed (the orphaned-nextest shape) — dead means the TREE is dead, which only a process-group kill can say",
}, function(t)
  local token = "87.331"
  -- The shell backgrounds a grandchild, then hangs silently past the idle bound. Both live in
  -- the conduct's group; the idle kill must take both.
  shell.run({ prova.bin, "eval", [[
pcall(function()
  return shell.run("(sleep 87.331 &); sleep 30", { idle_timeout = "400ms" })
end)
return "done"
]] }, { timeout = "20s" })
  local swept = wait_until(function() return not alive(token) end, 3)
  reap_stray(token)
  t:expect(swept, "the grandchild died with the shell"):is_true()
end)

--- A sandbox whose one proof conducts a long, UNBOUNDED sleep — the leased buffered path.
local function conducting_package(t, token)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/hold_test.lua",
    'prova.test("holds", function(t)\n  shell.run("sleep ' .. token .. '")\n  t:expect(true):is_true()\nend)\n')
  return proj
end

prova.test("kill -9 on prova sweeps its leased conducts — the death no cleanup code survives", {
  covers = "docs/design/verifiers.md#conduct-lease-survives-prova-death",
  proves = "SIGKILL runs no destructors, no signal handlers, nothing — if the conduct dies anyway, the lease is genuinely held OUTSIDE the dying process: the reaper's pipe EOF is kernel-delivered for every death, so this is the strongest case, and Ctrl-C/SIGTERM are strictly easier",
}, function(t)
  local token = "88.442"
  local proj = conducting_package(t, token)
  local inner = shell.spawn({ prova.bin }, { cwd = proj })

  t:expect(wait_until(function() return alive(token) end, 15), "the conduct is running"):is_true()
  shell.run("kill -9 " .. inner.pid)
  local swept = wait_until(function() return not alive(token) end, 5)
  reap_stray(token)
  inner:wait()
  t:expect(swept, "the reaper swept the leased group after a SIGKILL"):is_true()
end)

prova.test("an interrupted prova takes its conducts with it — Ctrl-C stays whole", {
  covers = "docs/design/verifiers.md#conduct-lease-survives-prova-death",
  proves = "the trap that stalled the naive fix: conducts in their own groups stop hearing the terminal's SIGINT, so the lease must make interrupt behavior stronger than the shared-group accident it replaces — prova dies of SIGINT, the pipe closes, the sweep happens",
}, function(t)
  local token = "89.553"
  local proj = conducting_package(t, token)
  local inner = shell.spawn({ prova.bin }, { cwd = proj })

  t:expect(wait_until(function() return alive(token) end, 15), "the conduct is running"):is_true()
  shell.run("kill -INT " .. inner.pid)
  local swept = wait_until(function() return not alive(token) end, 5)
  reap_stray(token)
  inner:wait()
  t:expect(swept, "the conduct died with the interrupted prova"):is_true()
end)

prova.test("`prova start` provisions hold no lease: the topology outlives the invocation", {
  covers = "docs/design/verifiers.md#detached-topologies-hold-no-lease",
  proves = "the lease's premise — conducts die with the run — is exactly wrong for the verb whose purpose is outliving it: were start's spawns leased, the reaper would kill the topology the moment start exits, and the carve-out is what makes the two claims compatible",
}, function(t)
  local token = "91.664"
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/plugins")
  fs.write(proj .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
svc = "plugins/svc.lua"

[topologies]
daemon = { package = "svc", factory = "daemon" }
]])
  fs.write(proj .. "/plugins/svc.lua", [[
local M = {}
function M.daemon(ctx)
  local proc = shell.spawn("sleep ]] .. token .. [[")
  return { svc = { url = "proc://" .. tostring(proc.pid) } }
end
return M
]])

  local started = shell.run({ prova.bin, "start", "daemon" }, { cwd = proj, merge_stderr = true, timeout = "30s" })
  t:expect(started.code, started.stdout):equals(0)
  -- `start` has exited; a leased spawn would be swept by its reaper right here.
  shell.run("sleep 0.5")
  local survived = alive(token)
  shell.run({ prova.bin, "down", "daemon" }, { cwd = proj, merge_stderr = true, timeout = "30s" })
  reap_stray(token)
  t:expect(survived, "the detached topology outlived the invocation"):is_true()
end)
