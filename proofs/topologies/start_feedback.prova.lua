--- `prova start` says what `prova up` says
--- (docs/design/agent-ergonomics.md#start-shows-what-up-shows).
---
--- The field report: `start` printed one line and then sat. Everything the holder was doing —
--- image pulls, builds, readiness waits — went to `<var>/running/<name>.log`, which nobody was
--- watching, because the child's stdio is redirected there precisely so it can outlive the
--- invocation. For a kind cluster that is minutes of cursor, and a cursor is indistinguishable
--- from a wedge; it is the exact silence the activity renderer exists to end
--- (docs/plans/run-progress-feedback.md), unavailable in the one mode that needed it most.
---
--- The cure is a relay, not a second renderer: `start` streams the holder's log back to its own
--- stderr as it arrives. Which means the interesting assertions are about the SEAM — the holder's
--- activity must arrive, the endpoints must arrive exactly ONCE (relayed *and* printed would be
--- twice), the attached tail line must not arrive at all (a detached hold does not end at Ctrl-C),
--- and the log must survive being read.
---
--- Resourceless by construction: the pause is a `shell.run` that sleeps, which crosses the 400ms
--- activity threshold on the same code path a container pull takes, with no docker in the loop.

local PORT = "19997"
local URL = "http://127.0.0.1:" .. PORT

--- A package with one registered topology whose factory pauses long enough to be narrated.
local function slow_package(root)
  fs.mkdir(root .. "/proofs")
  fs.mkdir(root .. "/plugins")
  fs.write(root .. "/proofs/a_test.lua",
    'prova.test("the suite runs", function(t) t:expect(1):equals(1) end)\n')
  fs.write(root .. "/plugins/kitchen.lua", [[
local M = {}
function M.slow(ctx)
  shell.run("sleep 2")            -- past the activity threshold, on the pull path's code
  return { svc = { url = "]] .. URL .. [[" } }
end
return M
]])
  fs.write(root .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[dependencies]
kitchen = "plugins/kitchen.lua"

[topologies]
narrated = { package = "kitchen", factory = "slow" }
]])
end

--- How many times `needle` occurs in `haystack`, matched literally.
local function occurrences(haystack, needle)
  local _, n = haystack:gsub((needle:gsub("%p", "%%%0")), "")
  return n
end

prova.test("`prova start` relays the holder's activity while it comes up", {
  covers = "docs/design/agent-ergonomics.md#start-shows-what-up-shows",
  proves = "detached mode was the one place the activity renderer's output could not be seen — the holder narrates into a log file opened so it can outlive the invocation, and `start` then waited in silence for however long the stack took. A minutes-long silence is not distinguishable from a hang, which is the whole reason the renderer exists",
}, function(t)
  local root = t:tempdir("relay") .. "/pkg"
  slow_package(root)

  local r = shell.run({ prova.bin, "start", "narrated" },
    { cwd = root, merge_stderr = true, timeout = "180s" })
  t:defer(function()
    shell.run({ prova.bin, "down", "narrated" }, { cwd = root, merge_stderr = true, timeout = "60s" })
  end)

  t:expect(r.code, r.stdout):equals(0)
  -- The holder's own narration, in the caller's own stream. Same line an attached `up` prints.
  t:expect(r.stdout, "the holder's activity reached the caller"):contains("sleep 2")
  t:expect(r.stdout, "…including the line that resolves it, not just the one that opens it")
    :contains("done in")
  -- And the endpoints, which `start` prints itself.
  t:expect(r.stdout, "the endpoints are reported"):contains(URL)
end)

prova.test("the relay stops at the endpoints, so they are reported exactly once", {
  covers = "docs/design/agent-ergonomics.md#start-shows-what-up-shows",
  proves = "the log carries BOTH of the holder's streams, so a relay that simply echoed it would print the endpoint block a second time — and would print it on stderr, moving the endpoints off the stdout where anything piping `prova start` has always found them",
}, function(t)
  local root = t:tempdir("once") .. "/pkg"
  slow_package(root)

  local split = shell.run({ prova.bin, "start", "narrated" }, { cwd = root, timeout = "180s" })
  t:defer(function()
    shell.run({ prova.bin, "down", "narrated" }, { cwd = root, merge_stderr = true, timeout = "60s" })
  end)

  t:expect(split.code, split.stderr):equals(0)
  t:expect(occurrences(split.stdout .. split.stderr, URL), "one endpoint block, not two"):equals(1)
  -- Streams kept apart: the endpoints are stdout's (a caller pipes them), the narration stderr's.
  t:expect(split.stdout, "the endpoint block stays on stdout"):contains(URL)
  t:expect(split.stderr, "the relayed activity stays on stderr"):contains("sleep 2")
  t:expect(split.stderr, "…and never carries the endpoints"):never():contains(URL)
  -- The attached tail line is the one thing that must NOT be relayed: this hold does not end at
  -- Ctrl-C, it ends at `prova down`, and the footer says so.
  t:expect(split.stdout .. split.stderr, "a detached hold is not held by this terminal")
    :never():contains("Ctrl-C to tear down")
  t:expect(split.stdout, "…the footer names the verb that actually stops it"):contains("prova down")
end)

prova.test("relaying is non-destructive — the log is still the whole record", {
  covers = "docs/design/agent-ergonomics.md#start-shows-what-up-shows",
  proves = "`start` names the log so a reader has something to follow after it exits; a relay that consumed what it echoed would leave that promise pointing at a file missing everything already read",
}, function(t)
  local root = t:tempdir("log") .. "/pkg"
  slow_package(root)

  local r = shell.run({ prova.bin, "start", "narrated" },
    { cwd = root, merge_stderr = true, timeout = "180s" })
  t:defer(function()
    shell.run({ prova.bin, "down", "narrated" }, { cwd = root, merge_stderr = true, timeout = "60s" })
  end)
  t:expect(r.code, r.stdout):equals(0)

  local log_path = root .. "/.prova/var/running/narrated.log"
  t:expect(r.stdout, "…and `start` says where it is"):contains("narrated.log")
  local log = fs.read(log_path)
  t:expect(log, "the narration is still there"):contains("sleep 2")
  t:expect(log, "…and so is the endpoint block the relay declined to echo"):contains(URL)
end)
