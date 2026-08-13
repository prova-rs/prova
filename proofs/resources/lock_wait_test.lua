-- Contention is observable (docs/design/agent-ergonomics.md#narrate-lock-waits): every seam that
-- can wait says so WITH its duration, and a run banks the wall time contention actually cost it as
-- `run.lock_wait_ms`. Four seams, deliberately different in kind:
--   1. the scheduler's queue        — outside the waiting unit's duration
--   2. the `Scope.Run` single-flight — inside the reader's duration
--   3. a `[runner] locks` provision — a blocking hold before the build
--   4. `prova lock … -- cmd`        — the wrapper, which said "waiting" and never "for how long"
--
-- A lock wait cannot be simulated in-process, so the holder here is a real second prova
-- (`prova lock`, machine-scoped so two independent packages contend). Every hold outlasts the
-- narration threshold (400ms) so no assertion depends on a race.

--- A machine-wide token unique to this test, so a concurrent agent's run can never contend with
--- ours (machine locks live in a box-wide directory — a fixed name would be a shared secret).
local function token(t, suffix)
  return "provawait-" .. t:tempdir():match("([^/\\]+)$") .. "-" .. suffix
end

--- Hold `tok` for `seconds` in a detached prova, and return a stopper. The wrapper verb IS seam 4.
local function holder(tok, seconds)
  local proc = shell.spawn({ prova.bin, "lock", tok, "--machine", "--", "sleep", tostring(seconds) })
  shell.run("sleep 0.3") -- let it acquire before the contender starts
  return proc
end

local function pkg_with(t, manifest, proofs)
  local dir = t:tempdir() .. "/pkg"
  fs.mkdir(dir .. "/proofs")
  fs.write(dir .. "/prova.toml", manifest)
  for name, body in pairs(proofs) do
    fs.write(dir .. "/proofs/" .. name, body)
  end
  return dir
end

local function banked(dir, name)
  local record = json.decode(fs.read(dir .. "/.prova/var/last-run.json"))
  for _, m in ipairs(record.measurements or {}) do
    if m.name == name then return m.value end
  end
  return nil
end

prova.test("a queued leaf narrates its wait, naming the token and how long", {
  covers = "docs/design/agent-ergonomics.md#narrate-lock-waits",
  proves = "the field report's cost: a conduct read 848.8s wall for ~190s of work, and the operator diagnosed it by cross-referencing a sibling invocation's logs. A queued conduct and a slow one are indistinguishable unless the queue says its own name and duration",
}, function(t)
  local tok = token(t, "idle")
  local hold = holder(tok, 2)
  local dir = pkg_with(t, '[run]\nproofs = ["proofs"]\n', {
    ["needs_test.lua"] = 'prova.test("needs the token", { locks = { prova.writes("' .. tok
      .. '", { scope = "machine" }) } }, function(t) t:expect(true):is_true() end)\n',
  })

  local r = shell.run({ prova.bin }, { cwd = dir, merge_stderr = true, timeout = "120s" })
  hold:wait()
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the wait is narrated"):contains("waiting for")
  t:expect(r.stdout, "…naming the token"):contains(tok)
  t:expect(r.stdout:match("[%d%.]+s") ~= nil, "…with a duration:\n" .. r.stdout):is_true()
end)

prova.test("a wait overlapped with other work is narrated too — busy is not the same as unblocked", {
  covers = "docs/design/agent-ergonomics.md#narrate-lock-waits",
  proves = "the gap the old narration left: it fired only when the run had nothing else in flight, so under -j the very case a multi-agent harness creates — one leaf progressing while another queues — reported nothing at all, and the queued leaf's cost was invisible even in principle",
}, function(t)
  local tok = token(t, "busy")
  local hold = holder(tok, 2)
  local dir = pkg_with(t, '[run]\nproofs = ["proofs"]\n', {
    -- The busy leaf OUTLASTS the hold, so the run is never idle while the other leaf queues —
    -- which is precisely the case the old idle-only narration could not see. (A shorter busy leaf
    -- passes for the wrong reason: the run goes idle before the hold ends and the old line fires.)
    ["busy_test.lua"] = 'prova.test("busy elsewhere", function(t) prova.sleep(4000) t:expect(true):is_true() end)\n',
    ["needs_test.lua"] = 'prova.test("needs the token", { locks = { prova.writes("' .. tok
      .. '", { scope = "machine" }) } }, function(t) t:expect(true):is_true() end)\n',
  })

  local r = shell.run({ prova.bin, "-j", "2" }, { cwd = dir, merge_stderr = true, timeout = "120s" })
  hold:wait()
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the overlapped wait is narrated:\n" .. r.stdout):contains("waiting for")
  t:expect(r.stdout, "…naming the token"):contains(tok)
end)

prova.test("the run banks run.lock_wait_ms — stalled wall time, and a zero when nothing waited", {
  covers = "docs/design/agent-ergonomics.md#narrate-lock-waits",
  proves = "narration dies with the scrollback; the datum a multi-agent harness needs — is contention now most of the sweep? — must survive into the record where a reminder condition and a baseline read it, exactly as run.duration_ms does. And it must be banked ALWAYS: a metric absent when nothing happened is indistinguishable from one never measured, so a baseline cannot hold it",
}, function(t)
  local tok = token(t, "banked")
  local hold = holder(tok, 2)
  local contended = pkg_with(t, '[run]\nproofs = ["proofs"]\n', {
    ["needs_test.lua"] = 'prova.test("needs the token", { locks = { prova.writes("' .. tok
      .. '", { scope = "machine" }) } }, function(t) t:expect(true):is_true() end)\n',
  })
  shell.run({ prova.bin }, { cwd = contended, merge_stderr = true, timeout = "120s" })
  hold:wait()

  local quiet = pkg_with(t, '[run]\nproofs = ["proofs"]\n', {
    ["quiet_test.lua"] = 'prova.test("a token nobody wants", { locks = { prova.writes("' .. token(t, "quiet")
      .. '", { scope = "machine" }) } }, function(t) t:expect(true):is_true() end)\n',
  })
  local uncontended = shell.run({ prova.bin }, { cwd = quiet, merge_stderr = true, timeout = "120s" })
  t:expect(uncontended.stdout, "an uncontended run says nothing about waiting"):never():contains("waiting for")

  local stalled, zero = banked(contended, "run.lock_wait_ms"), banked(quiet, "run.lock_wait_ms")
  t:expect(stalled, "the contended run banked its stall"):is_truthy()
  t:expect(stalled > 500, "…and it is the wall time it lost: " .. tostring(stalled) .. "ms"):is_true()
  t:expect(zero, "the uncontended run banked the metric anyway"):is_truthy()
  t:expect(zero, "…as a measured zero, not an absence"):equals(0)
end)

prova.test("`prova lock` reports what it waited, so the wrapper is not the blind spot", {
  covers = "docs/design/agent-ergonomics.md#narrate-lock-waits",
  proves = "the wrapper exists so non-prova tools (xtask, a Makefile, a CI step) can join the house rule — and it is exactly where an operator watches a queued build; it said 'waiting for lock (held elsewhere)…' and never how long, which is the report the field note complained about, verbatim",
}, function(t)
  local tok = token(t, "verb")
  local hold = holder(tok, 2)
  local r = shell.run({ prova.bin, "lock", tok, "--machine", "--", "true" },
    { merge_stderr = true, timeout = "60s" })
  hold:wait()
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the wrapper says it waited"):contains("waited")
  t:expect(r.stdout:match("[%d%.]+s") ~= nil, "…with a duration:\n" .. r.stdout):is_true()
end)

prova.test("a Scope.Run reader narrates its single-flight wait — the seam inside a duration", {
  covers = "docs/design/agent-ergonomics.md#narrate-lock-waits",
  proves = "this is the only seam that lands INSIDE the waiting unit's own duration, which is what makes a reader read 848.8s for 190s of work with no flock involved. It is not banked as lock wait (nothing is contended — the conducting worker is working), so narration is the only thing that can explain the number",
}, function(t)
  local dir = pkg_with(t, '[run]\nproofs = ["proofs"]\n\n[suites.a]\npaths = ["proofs/a"]\n\n[suites.b]\npaths = ["proofs/b"]\n', {})
  fs.mkdir(dir .. "/proofs/a")
  fs.mkdir(dir .. "/proofs/b")
  local shared = [[
local slow = prova.fixture("slow-conduct", Scope.Run, function()
  shell.run("sleep 1.5")
  return "conducted"
end)
prova.test("reads the conduct", function(t)
  t:expect(t:use(slow)):equals("conducted")
end)
]]
  fs.write(dir .. "/proofs/a/reader_test.lua", shared)
  fs.write(dir .. "/proofs/b/reader_test.lua", shared)

  local r = shell.run({ prova.bin, "-j", "2" }, { cwd = dir, merge_stderr = true, timeout = "120s" })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the waiting reader says what it is waiting for:\n" .. r.stdout)
    :contains("slow-conduct")
end)

prova.test("a `[runner]` provision that queues says so before it builds", {
  covers = "docs/design/agent-ergonomics.md#narrate-lock-waits",
  proves = "the provision is the FIRST thing a run does, so a silent blocking hold here reads as prova hanging before it has printed anything — the worst place in the run to be quiet, and the one seam whose wait precedes every other line of output",
}, function(t)
  local tok = token(t, "provision")
  local dir = t:tempdir() .. "/pkg"
  fs.mkdir(dir .. "/proofs")
  fs.mkdir(dir .. "/src")
  fs.mkdir(dir .. "/bin")
  fs.write(dir .. "/src/marker.txt", "v1\n")
  fs.write(dir .. "/proofs/subject_test.lua",
    'prova.test("the subject exists", function(t) t:expect(prova.bin):is_truthy() end)\n')
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[runner]',
    "build   = 'cp \"$PROVA_SRC\" bin/prova'",
    'bin     = "bin/prova"',
    'sources = ["src"]',
    'locks   = ["' .. tok .. '"]',
  }, "\n"))

  -- The provision's lock is package-scoped, so the holder must contend at THIS home.
  local hold = shell.spawn({ prova.bin, "lock", tok, "--", "sleep", "2" }, { cwd = dir })
  shell.run("sleep 0.3")
  -- `PROVA_RUN_DEPTH = ""` re-arms provisioning inside a nested run (a nested prova skips the
  -- provision by design); `PROVA_SRC` is what this sandbox's build copies into place.
  local r = shell.run({ prova.bin }, {
    cwd = dir, merge_stderr = true, timeout = "120s",
    env = { PROVA_RUN_DEPTH = "", PROVA_SRC = prova.bin },
  })
  hold:wait()

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the provision's wait is narrated:\n" .. r.stdout):contains("waited")
  t:expect(r.stdout, "…naming the token"):contains(tok)
  t:expect(r.stdout, "…and what it was waiting to do"):contains("provisioning the subject")
end)
