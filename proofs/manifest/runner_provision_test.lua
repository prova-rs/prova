-- `[runner]` names the SUBJECT, not the conductor
-- (docs/design/manifest.md#runner-is-the-subject-not-the-conductor). Nothing re-execs: the
-- binary you invoke answers as itself — your installed prova stays the tool in your hand for
-- queries, MCP, everything. A RUN is testing, so it provisions the declared subject just in
-- time and injects it as `prova.bin`; the sandbox build here appends to a log, so every
-- provision is a countable fact.

local function sandbox(t)
  local dir = t:tempdir()
  fs.mkdir(dir .. "/proofs")
  fs.mkdir(dir .. "/src")
  fs.mkdir(dir .. "/bin")
  fs.write(dir .. "/src/marker.txt", "v1\n")
  -- The sandbox suite records which binary its proofs would drive recursively.
  fs.write(dir .. "/proofs/subject_test.lua",
    'prova.test("who is the subject", function(t)\n' ..
    '  fs.write(prova.root .. "/subject.txt", prova.bin)\n' ..
    '  t:expect(true):is_true()\nend)\n')
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[runner]',
    "build   = 'echo built >> build.log && cp \"$PROVA_SRC\" bin/prova'",
    'bin     = "bin/prova"',
    'sources = ["src"]',
  }, "\n"))
  return dir
end

local function invoke(dir, args)
  -- Re-arm provisioning inside the proof sandbox: empty-string guards count as unset. Both
  -- run-scoped variables are cleared, for the same reason — this sandbox is asking what a package's
  -- OWN manifest resolves to, so anything the ambient conduct set about depth or subject would
  -- answer for it. `PROVA_SUBJECT_BIN` is set for real by the coverage conduct, which is where
  -- this stopped being hypothetical: these proofs failed under `prova run coverage` and passed
  -- everywhere else, because only that conduct exports it.
  return shell.run({ prova.bin, table.unpack(args) }, {
    cwd = dir, timeout = "120s", merge_stderr = true,
    env = { PROVA_RUN_DEPTH = "", PROVA_SUBJECT_BIN = "", PROVA_SRC = prova.bin },
  })
end

local function provisions(dir)
  local ok, text = pcall(fs.read, dir .. "/build.log")
  if not ok then return 0 end
  local n = 0
  for _ in text:gmatch("built") do n = n + 1 end
  return n
end

prova.test("a run provisions the subject and injects it as prova.bin — the conductor never re-execs", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "the re-exec trampoline taxed every invocation with a build the verb never needed — an MCP handshake died behind one, live; a run is the only thing that is TESTING, so it is the only thing that provisions",
}, function(t)
  local dir = sandbox(t)
  local r = invoke(dir, {})
  t:expect(r.code, r.stdout):equals(0)
  t:expect(provisions(dir), "the run provisioned the subject"):equals(1)
  -- The suite's nested reach IS the declared subject — not the (different) conductor binary.
  t:expect(fs.read(dir .. "/subject.txt"), "prova.bin is the subject"):contains("bin/prova")

  -- Freshness counts the bin's own mtime: nothing changed, so a second run does not rebuild.
  local again = invoke(dir, {})
  t:expect(again.code):equals(0)
  t:expect(provisions(dir), "fresh subject, no rebuild"):equals(1)

  -- Sources move on; the next run re-provisions on its own.
  prova.sleep(1100) -- mtime granularity
  fs.write(dir .. "/src/marker.txt", "v2\n")
  invoke(dir, {})
  t:expect(provisions(dir), "a stale subject rebuilds for a run"):equals(2)
end)

prova.test("-U leaves a fresh provision untouched; --reprovision is the provision's own distrust", {
  covers = "docs/design/manifest.md#provision-refresh-respelling",
  proves = "a provision is a build product of the working tree, not a cached remote asset — a run under -U that also rebuilt the subject was paying a build for a flag about caches, and the flag that names the distrust must name which thing is distrusted",
}, function(t)
  local dir = sandbox(t)
  invoke(dir, {})
  t:expect(provisions(dir)):equals(1)

  invoke(dir, { "-U" })
  t:expect(provisions(dir), "-U does not touch a fresh provision"):equals(1)

  invoke(dir, { "--reprovision" })
  t:expect(provisions(dir), "--reprovision rebuilds even when fresh"):equals(2)
end)

prova.test("queries and `prova mcp` never provision — the tool in your hand answers as itself", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "re-provisioning a runner to answer `prova owed` taxed every ledger read in a self-hosting repo, and the same hop's build outran an MCP client's 30s handshake budget — navigation must be immediate; a human refreshes their tools deliberately",
}, function(t)
  local dir = sandbox(t)
  -- No subject has ever been built; the ledger read answers anyway, building nothing.
  local q = invoke(dir, { "specs", "--backlog" })
  t:expect(q.code, q.stdout):equals(0)
  t:expect(provisions(dir), "no build for a ledger read"):equals(0)

  -- The MCP server starts (and EOF-exits) without a provision in the handshake path.
  -- An immediate EOF is a failed handshake from the SERVER's point of view (non-zero is its
  -- honest answer); what this proof pins is that it got to answer AT ALL without a provision.
  local m = shell.run("printf '' | " .. prova.bin .. " mcp", {
    cwd = dir, timeout = "60s", merge_stderr = true, env = { PROVA_RUN_DEPTH = "" },
  })
  t:expect(m.stdout, "the server reached the handshake, not a build"):contains("initialize")
  t:expect(provisions(dir), "no build in the handshake path"):equals(0)
end)

--- Drive the sandbox's `prova mcp` as a real conversation over stdio: handshake, then each tool
--- call awaited before the next is written. The server is LONG-LIVED here on purpose — one process
--- across every call, the way a client holds it, so staleness between calls is observable.
local function mcp_session(t, dir)
  local sess = stdio.spawn(t, {
    cmd = { prova.bin, "mcp" },
    cwd = dir,
    -- The same re-arm `invoke` performs: the sandbox asks what its OWN manifest resolves to.
    env = { PROVA_RUN_DEPTH = "", PROVA_SUBJECT_BIN = "", PROVA_SRC = prova.bin },
    framing = "line",
    codec = "json",
  })
  sess:send({
    jsonrpc = "2.0", id = 1, method = "initialize",
    params = { protocolVersion = "2024-11-05", capabilities = {},
      clientInfo = { name = "proof", version = "0" } },
  })
  sess:recv({ where = { id = 1 }, timeout = "60s" })
  sess:send({ jsonrpc = "2.0", method = "notifications/initialized" })
  local id = 1
  -- A tool call's text (JSON on success, the refusal's prose on error) and its isError flag.
  -- `recv` decodes EVERY line it passes over, so a stray non-JSON line on the server's stdout
  -- raises here — the protocol stream's integrity is asserted by every call, not just one.
  return sess, function(tool, arguments)
    id = id + 1
    sess:send({ jsonrpc = "2.0", id = id, method = "tools/call",
      params = { name = tool, arguments = arguments or {} } })
    local resp = sess:recv({ where = { id = id }, timeout = "120s" })
    return resp.result.content[1].text, resp.result.isError
  end
end

prova.test("an MCP `run` is a run: it provisions the subject, and a long-lived server never judges a stale one", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "field report 2026-09-18: the MCP `run` tool never provisioned — after an engine edit it "
    .. "returned in 203ms, judging the previous target/debug/prova, and reported a just-fixed proof "
    .. "red with the pre-fix message while the CLI rebuilt and went green; a server outlives every "
    .. "edit, so it is the transport where a stale subject is the steady state",
}, function(t)
  local dir = sandbox(t)
  local sess, call = mcp_session(t, dir)
  t:expect(provisions(dir), "the handshake built nothing"):equals(0)

  -- A query answers as the server itself, immediately — still no build.
  call("owed")
  t:expect(provisions(dir), "a query tool never provisions"):equals(0)

  local text, is_err = call("run")
  t:expect(is_err, text):is_falsy()
  t:expect(json.decode(text).passed, text):equals(1)
  t:expect(provisions(dir), "the run provisioned the subject"):equals(1)
  t:expect(fs.read(dir .. "/subject.txt"), "prova.bin is the subject"):contains("bin/prova")

  call("run")
  t:expect(provisions(dir), "a fresh subject is not rebuilt"):equals(1)

  -- The sources move on under the SAME server process; its next run notices on its own.
  prova.sleep(1100) -- mtime granularity
  fs.write(dir .. "/src/marker.txt", "v2\n")
  call("run")
  t:expect(provisions(dir), "the long-lived server re-provisions a stale subject"):equals(2)

  sess:eof()
  sess:wait({ timeout = "60s" })
end)

prova.test("the build's stdout never reaches a run's stdout — the MCP protocol stream or a --format json report", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "a provision inside the MCP server shares the server's stdout, which IS the JSON-RPC "
    .. "channel: one line of build chatter there is a corrupt frame, and the same leak splices "
    .. "into a CLI run's machine-readable report — the build's output belongs on stderr, visible "
    .. "and out of band",
}, function(t)
  local dir = sandbox(t)
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[runner]',
    "build   = 'echo build-chatter-on-stdout && echo built >> build.log && cp \"$PROVA_SRC\" bin/prova'",
    'bin     = "bin/prova"',
    'sources = ["src"]',
  }, "\n"))

  local sess, call = mcp_session(t, dir)
  local text, is_err = call("run")   -- raises if the chatter landed on the protocol stream
  t:expect(is_err, text):is_falsy()
  t:expect(provisions(dir), "the chatty build did run"):equals(1)
  t:expect(sess:stderr(), "the chatter was routed to stderr, not swallowed"):contains("build-chatter-on-stdout")
  sess:eof()
  sess:wait({ timeout = "60s" })

  -- The CLI half: stdout carries the reporter, so it must carry nothing else.
  local r = shell.run({ prova.bin, "--reprovision", "--format", "json" }, {
    cwd = dir, timeout = "120s",
    env = { PROVA_RUN_DEPTH = "", PROVA_SUBJECT_BIN = "", PROVA_SRC = prova.bin },
  })
  t:expect(r.code, r.stderr):equals(0)
  t:expect(provisions(dir), "--reprovision rebuilt"):equals(2)
  t:expect(r.stdout, "the report stream is the report alone"):never():contains("build-chatter")
  t:expect(r.stderr, "the build's output is still visible"):contains("build-chatter-on-stdout")
end)

prova.test("a failed provision over MCP is a refusal that names the build — nothing judges", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "the CLI's failed provision is exit 2 with the reason on stderr, but a server's stderr "
    .. "is invisible to its client — the refusal must carry what failed and how to reproduce it, "
    .. "or the agent sees an error with nothing to act on",
}, function(t)
  local dir = sandbox(t)
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[runner]',
    'build   = "echo attempted >> build.log && exit 1"',
    'bin     = "bin/prova"',
    'sources = ["src"]',
  }, "\n"))
  local sess, call = mcp_session(t, dir)
  local text, is_err = call("run")
  t:expect(is_err, "a failed provision is an error result"):is_truthy()
  t:expect(text, "the refusal says what failed"):contains("build failed")
  t:expect(text, "…and names the command to reproduce it"):contains("echo attempted >> build.log")
  t:expect(fs.exists(dir .. "/subject.txt"), "no proof body ran"):is_false()
  sess:eof()
  sess:wait({ timeout = "60s" })
end)

prova.test("a failed provision is loud (exit 2) and nothing judges", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "a build failure is a failed provision, not a verdict — and never a silent run against whatever subject happened to be lying around",
}, function(t)
  local dir = sandbox(t)
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[runner]',
    'build   = "echo attempted >> build.log && exit 1"',
    'bin     = "bin/prova"',
    'sources = ["src"]',
  }, "\n"))
  local r = invoke(dir, {})
  t:expect(r.code, "a failed provision exits 2"):equals(2)
  t:expect(r.stdout):contains("build failed")
  t:expect(fs.exists(dir .. "/subject.txt"), "no proof body ran"):is_false()
end)

prova.test("a nested prova.bin child never re-provisions under a live suite", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "the guard env inherits to every descendant — no rebuild storm underneath the very suite the subject is running",
}, function(t)
  local dir = sandbox(t)
  -- No re-arm: this child inherits the real proof context's PROVA_RUN_DEPTH, exactly as a
  -- nested run inside a proof would.
  local r = shell.run({ prova.bin, "--allow-empty" }, {
    cwd = dir, timeout = "120s", merge_stderr = true, env = { PROVA_SRC = prova.bin },
  })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(provisions(dir), "no provision under the guard"):equals(0)
end)

prova.test("the provision holds the manifest-declared locks — a build can never race a conduct", {
  covers = "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
  proves = "the provision IS a cargo invocation, so it must join the same house rule the suite's conducts encode — an unlocked provision racing a proof holding writes(\"cargo\") was the loophole's last corner; the lock file is the contract, so xtask and any external tool join by flocking the same path",
}, function(t)
  local dir = sandbox(t)
  fs.write(dir .. "/prova.toml", table.concat({
    '[run]', 'proofs = ["proofs"]', '',
    '[runner]',
    "build   = 'printf \"begin\\n\" >> build.log && sleep 0.4 && printf \"end\\n\" >> build.log && cp \"$PROVA_SRC\" bin/prova'",
    'bin     = "bin/prova"',
    'sources = ["src"]',
    'locks   = ["build-slot"]',
  }, "\n"))

  -- Two concurrent FORCED provisions (--reprovision): without the lock their begin/end marks interleave;
  -- under it, each build owns its critical section whole.
  local r = shell.run({
    "sh", "-c",
    '"$0" --reprovision --allow-empty > /dev/null 2>&1 & "$0" --reprovision --allow-empty > /dev/null 2>&1; wait',
    prova.bin,
  }, {
    cwd = dir, timeout = "180s", merge_stderr = true,
    env = { PROVA_RUN_DEPTH = "", PROVA_SRC = prova.bin },
  })
  t:expect(r.code, r.stdout):equals(0)

  local lines = {}
  for line in fs.read(dir .. "/build.log"):gmatch("[^\n]+") do lines[#lines + 1] = line end
  t:expect(#lines, "both provisions ran whole"):equals(4)
  t:expect(lines[1]):equals("begin")
  t:expect(lines[2], "the first build finished before the second began"):equals("end")
  t:expect(lines[3]):equals("begin")
  t:expect(lines[4]):equals("end")
end)

--- `PROVA_SUBJECT_BIN` names the subject outright, ahead of any declared `[runner]`, and every
--- descendant inherits it.
---
--- The one caller is coverage: the layered conduct runs the suite through an INSTRUMENTED build
--- and needs `prova.bin` children to be that same build, because the recursion is where the
--- runtime executes and therefore most of what the layer measures. Before this existed the conduct
--- set `PROVA_TRAMPOLINED`, which nothing read — it named a re-exec mechanism that had been
--- retired — so the subject silently stayed the ordinary uninstrumented `target/debug/prova`,
--- every child contributed no profile data, and the layer read 45% against a 69% floor with no
--- coverage actually lost. Nothing failed; a number simply became untrue and the ratchet took the
--- blame for four days.
---
--- Inheritance is the load-bearing half, and the reason this is an env var rather than a flag: the
--- recursion is arbitrarily deep, so a flag would reach the first child and stop.
prova.test("PROVA_SUBJECT_BIN names the subject, and the whole recursion inherits it", {
  covers = {
    "docs/design/manifest.md#runner-is-the-subject-not-the-conductor",
    -- The executable half of the claim: the conduct's intent is now READ by something, which is
    -- exactly what the retired `PROVA_TRAMPOLINED` was not.
    "docs/design/agent-ergonomics.md#a-measurement-must-prove-it-measured",
  },
  proves = "the coverage conduct depends on prova.bin children being ITS build rather than the manifest's; when the variable carrying that intent was read by nobody, the layer went on producing a number that no longer meant what it said — a silent wrong answer, not a failure",
}, function(t)
  -- A marker path that is never executed: the grandchild is spawned through the real binary
  -- explicitly, so this proves RESOLUTION without requiring the marker to be runnable.
  local marker = t:tempdir("subject-marker") .. "/declared-subject"
  local real = prova.bin
  -- Each snippet ends in `return nil` so `eval` prints nothing of its own — otherwise the value of
  -- the trailing `io.write` (a file handle) lands in the middle of what is being asserted on.
  local report = 'io.write(prova.bin); return nil'
  local inner = string.format('io.write("<<", prova.bin, ">>"); return nil')
  local snippet = string.format(
    'local r = shell.run({ %q, "eval", %q }); io.write("child=", prova.bin, " grand=", r.stdout); '
    .. 'return nil', real, inner)

  local r = shell.run({ real, "eval", snippet },
    { env = { PROVA_SUBJECT_BIN = marker }, merge_stderr = true })
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout, "the child resolves the override, not the declared [runner]")
    :contains("child=" .. marker)
  t:expect(r.stdout, "…and so does ITS child — the variable rides the whole recursion")
    :contains("grand=<<" .. marker .. ">>")

  -- Cleared, this repo's own `[runner]` wins. Compared against the DECLARED path rather than
  -- against `real`: the two are the same binary in an ordinary run, but under `prova run coverage`
  -- `prova.bin` is already the instrumented build, so asserting "unchanged from real" would be
  -- asserting the override is still in force — the opposite of the point. Empty counts as unset,
  -- matching `PROVA_RUN_DEPTH`, so this covers both spellings of "no override".
  local declared = prova.root .. "/target/debug/prova"
  local cleared = shell.run({ real, "eval", report },
    { env = { PROVA_SUBJECT_BIN = "" }, merge_stderr = true })
  t:expect(cleared.stdout, "an emptied variable is not an override — the manifest decides")
    :equals(declared)
  t:expect(cleared.stdout, "and it is emphatically not the marker"):never():equals(marker)
end)
