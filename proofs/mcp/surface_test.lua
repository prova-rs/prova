--- Black-box surface of MCP mode — the parity contract, the warm holder, and the one embedded
--- skill — driven over real stdio JSON-RPC against `prova.bin`. The deep end-to-end battery
--- lives in `crates/prova-cli/selftest/`; these proofs pin the CLAIMS, from the ledger's side.
---
--- The contract (docs/design/mcp-mode.md): the tool surface mirrors the CLI, doors included;
--- a server-held topology reports through `status {}` and never writes a `running/` record;
--- `run { topology }` resolves the held instance instead of provisioning (and never provisions
--- implicitly); `eval` is the same one-shot execution path on both transports; and one skill
--- document is served by `prova skill`, MCP `instructions`, and `skill --install`.

local scratch = prova.fixture("mcp-surface-scratch", Scope.File, function(ctx)
  -- Each call names its own directory, so asking twice for "1" is the same place and
  -- the scratch tree on disk says which sandbox is which.
  local nth = 0
  return function()
    nth = nth + 1
    return ctx:tempdir(tostring(nth))
  end
end)

-- The shared deputy RECIPE loads with the file (registration must precede the run's plan; a
-- require inside a test body would register after the fixture registry is sealed). Registering
-- is free — on a bare run nothing uses it, so nothing conducts.
local deputies = require("deputies")

--- Drive `prova mcp` as a real CONVERSATION: handshake, then each request written only after the
--- previous reply has come back, over `stdio.spawn` (docs/plans/stdio-transport.md).
---
--- This used to write every message into a `requests.jsonl` and redirect it in. That worked here
--- only because prova's MCP server happens to answer sequentially — it is a race in general, and
--- the same batch against a server free to dispatch concurrently answers turn two before turn one
--- has stored anything (`agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut`). These
--- proofs ASSERT the ordering they used to assume: `where = { id = … }` picks each reply out of
--- the stream, so a notification arriving mid-exchange is skipped rather than mistaken for it.
---
--- One process per call — ordering across tool calls IS the warmth.
local function mcp(t, dir, msgs, env)
  local sess = stdio.spawn(t, {
    cmd = { prova.bin, "mcp" },
    cwd = dir,
    env = env or {},
    framing = "line",   -- MCP over stdio is newline-delimited JSON
    codec = "json",
  })
  sess:send({
    jsonrpc = "2.0",
    id = 1,
    method = "initialize",
    params = {
      protocolVersion = "2024-11-05",
      capabilities = {},
      clientInfo = { name = "proof", version = "0" },
    },
  })
  local by_id = { [1] = sess:recv({ where = { id = 1 }, timeout = "60s" }) }
  sess:send({ jsonrpc = "2.0", method = "notifications/initialized" })

  for _, msg in ipairs(msgs) do
    sess:send(msg)
    -- A notification (no id) expects no answer; anything else is awaited before the next write.
    if msg.id then
      by_id[msg.id] = sess:recv({ where = { id = msg.id }, timeout = "120s" })
    end
  end
  -- Server shutdown is stdin EOF — the same signal a real client sends when it goes away, and
  -- what the held-topology teardown below hangs on.
  sess:eof()
  sess:wait({ timeout = "60s" })
  return by_id, sess
end

--- A tool result's decoded JSON payload plus its error flag.
local function tool_json(resp)
  return json.decode(resp.result.content[1].text), resp.result.isError
end

local function call(id, tool, arguments)
  return {
    jsonrpc = "2.0",
    id = id,
    method = "tools/call",
    params = { name = tool, arguments = arguments or {} },
  }
end

--- A package with one registered, resourceless topology whose factory counts provisions, defers
--- a teardown marker, and returns a mutable counter — warmth made observable from outside.
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
  local count = os.getenv("PROVA_PROOF_COUNT")
  if count then
    local n = fs.exists(count) and tonumber(fs.read(count)) or 0
    fs.write(count, tostring(n + 1))
  end
  local marker = os.getenv("PROVA_PROOF_MARKER")
  if marker then ctx:defer(function() fs.write(marker, "torn-down") end) end
  return { counter = { n = 0 }, svc = { url = "http://127.0.0.1:19999" } }
end
return M
]])
  fs.write(root .. "/proofs/warm_test.lua", [[
prova.test("accumulates in the held instance", function(t)
  local env = t:use("orders")
  env.counter.n = env.counter.n + 1
  fs.write(os.getenv("PROVA_PROOF_HITS"), tostring(env.counter.n))
  t:expect(env.counter.n):gte(1)
end)
]])
end

-- ── the deputed account, read across the suite boundary ─────────────────────────────────────

prova.test("a reader outside the ut suite binds to the deputy's account — one conduct, run-wide", {
  switch = "ut",
  requires = { "cargo-nextest" },
  locks = { prova.writes("cargo") },
  covers = "docs/design/verifiers.md#suite-scoped-shared-deputies",
  proves = "the dogfood of Scope.Run on the real workspace: this file is another suite — another Lua state, under -j another worker — and in `run all` it reads the SAME nextest conduct proofs/ut adopts. Before the fifth scope this read either re-paid the workspace compile or parsed an artifact with no ordering guarantee",
}, function(t)
  local report = junit.load(t:use(deputies.nextest))
  local case
  for _, c in ipairs(report.cases) do
    if c.name == "tests::mcp_tools_are_real_verbs" then case = c end
  end
  t:expect(case, "the CLI↔MCP parity unit gate is in the deputed account"):is_truthy()
  t:expect(case.outcome):equals("passed")
end)

-- ── the parity contract ──────────────────────────────────────────────────────────────────────

prova.test("the tool surface mirrors the CLI verbs, warm holder included",
  { covers = "docs/design/mcp-mode.md#mcp-cli-parity" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local by_id = mcp(t, root, { { jsonrpc = "2.0", id = 2, method = "tools/list" } })
  local names = {}
  for _, tool in ipairs(by_id[2].result.tools) do names[tool.name] = true end
  -- The FULL surface, not a sample: a tool silently dropped from the server would slip past a
  -- partial list (the gap query-consolidation increment 1 flagged). Lanes + account + drivers +
  -- the warm holder. `tests` is the tests lane (formerly `list`); `status` is the held-registry
  -- view (increment 7 reconciles its name with the CLI's `ps`).
  for _, expected in ipairs({
    "run", "tests", "specs", "reminders", "switches", "eval", "learn", "introspect",
    "capabilities", "packages", "attest", "evidence", "owed", "up", "down", "status",
  }) do
    t:expect(names[expected], "tool " .. expected .. " is served"):is_truthy()
  end
end)

-- ── verb parity: every tool name dispatches or teaches at the CLI ────────────────────────────

prova.test("every MCP tool name, typed at the CLI, dispatches or teaches — never a file error", {
  covers = "docs/design/mcp-mode.md#cli-mcp-verb-parity",
  proves = "an agent that learned the MCP names typed `prova status` and the run path read the verb as a filename (\"No such file or directory\") — a first-try miss across frontends; every tool name must answer as a verb or teach its CLI spelling",
}, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/one_test.lua",
    'prova.test("one", function(t) t:expect(true):is_true() end)\n')

  -- The LIVE tool surface, never a hand-kept list — a tool added tomorrow is swept tomorrow.
  local by_id = mcp(t, root, { { jsonrpc = "2.0", id = 2, method = "tools/list" } })
  local tools = by_id[2].result.tools
  t:expect(#tools, "the live surface, not a sample"):gte(15)

  -- Hermetic per-verb arguments where the bare spelling would leave the package: `packages`
  -- consults the built-in registry, and only --offline keeps the sweep off the network.
  local args = { packages = " --offline" }
  for _, tool in ipairs(tools) do
    local r = shell.run(prova.bin .. " " .. tool.name .. (args[tool.name] or ""),
      { cwd = root, merge_stderr = true, timeout = "120s" })
    t:expect(r.stdout, "`prova " .. tool.name .. "` answers as a verb or teaches")
      :never():contains("No such file or directory")
  end

  -- The two divergent spellings teach their CLI twin by name, and refuse rather than dispatch.
  local cap = shell.run(prova.bin .. " capture", { cwd = root, merge_stderr = true })
  t:expect(cap.code, "a teaching redirect refuses"):equals(2)
  t:expect(cap.stdout, "capture teaches its lane driver"):contains("specs capture")
  local st = shell.run(prova.bin .. " status", { cwd = root, merge_stderr = true })
  t:expect(st.code):equals(2)
  t:expect(st.stdout, "status teaches the detached-topology view"):contains("prova ps")
end)

-- ── the held registry: status, never ps ──────────────────────────────────────────────────────

--- A tempdir's basename, which is unique per call and survives the /private symlink macOS adds
--- when a path is canonicalized — so a reported package path can be matched against the directory
--- the proof created without the two spellings disagreeing.
local function leaf(path)
  return path:match("[^/]+$")
end

prova.test("a server-held topology reports through status and never writes a running record",
  { covers = "docs/design/mcp-mode.md#held-visible-via-status-not-ps" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  -- Hold it and ask status; no `down`, so the hold spans the batch. `prova ps` reads the
  -- `running/` records that `prova start` writes — a server-held topology must never mint one,
  -- which is exactly why it is invisible to `ps` and visible to `status`.
  local by_id = mcp(t, root, {
    call(2, "up", { name = "orders" }),
    call(3, "status"),
  })
  local report = tool_json(by_id[3])
  local held = report.held
  t:expect(held[1].name):equals("orders")
  t:expect(held[1].resources[1].url):equals("http://127.0.0.1:19999")
  -- Which holder, and whose package: a warm hold is the server's own, and it names the package
  -- its `up` resolved against — the two facts that tell an agent how to reach and reap it.
  t:expect(held[1].holder):equals("server")
  t:expect(held[1].package):contains(leaf(root))
  t:expect(report.packages[1], "the startup package is consulted by default"):contains(leaf(root))
  t:expect(report.note, "a package WAS consulted — nothing to warn about"):never():is_truthy()
  t:expect(fs.exists(root .. "/.prova/var/running/orders.json"),
    "no detached record was ever written"):equals(false)
end)

prova.test("status takes a package, and reports a DETACHED hold the server never provisioned", {
  covers = "docs/design/agent-ergonomics.md#mcp-status-cannot-be-aimed-at-a-package",
  proves = "a held ybor-studio-k8s was invisible to `status` from a server started in the user's home (where an agent harness's per-user MCP config puts it) while `prova --topology` attached to it warm from the package directory in the same minute; `{\"held\": []}` reads as 'safe to provision' and costs a cold stand-up of something already up",
}, function(t)
  local root = t:use(scratch)()
  registered(root)
  -- The wrong room, literally: a directory with no manifest at it or above it, which is what the
  -- server's startup resolution finds when its config is per-user rather than per-repo.
  local outside = t:use(scratch)()

  local started = shell.run(prova.bin .. " start orders", { cwd = root, merge_stderr = true })
  t:defer(function() shell.run(prova.bin .. " down orders", { cwd = root, merge_stderr = true }) end)
  t:expect(started.code, "the detached holder came up"):equals(0)

  local by_id = mcp(t, outside, {
    call(2, "status"),
    call(3, "status", { package = root }),
    -- An aim that misses is an error, never an empty list: `outside` is a real directory with no
    -- package in it, and answering `{held: []}` for it is the same wrong answer one level in.
    call(4, "status", { package = outside }),
  })

  -- From the wrong room the list is honestly empty AND says why — `packages` is what it read, so
  -- an empty one is the single case where `held: []` does not mean "nothing is up".
  local blind = tool_json(by_id[2])
  t:expect(#blind.held, "a server outside a package holds nothing itself"):equals(0)
  t:expect(#blind.packages, "and it consulted nothing"):equals(0)
  t:expect(blind.note, "the empty answer names its own limit"):contains("package")

  local aimed, aimed_err = tool_json(by_id[3])
  t:expect(aimed_err):never():is_truthy()
  t:expect(#aimed.held, "the same server, aimed, sees the hold"):equals(1)
  t:expect(aimed.held[1].name):equals("orders")
  t:expect(aimed.held[1].holder, "not this server's — a process's"):equals("detached")
  t:expect(aimed.held[1].pid):gt(0)
  t:expect(aimed.held[1].resources[1].url):equals("http://127.0.0.1:19999")
  t:expect(aimed.held[1].package):contains(leaf(root))
  t:expect(aimed.packages[1]):contains(leaf(root))

  t:expect(by_id[4].result.isError, "a package that resolves to nothing is refused"):is_truthy()
  t:expect(by_id[4].result.content[1].text):contains("no prova.toml found")
end)

-- ── one guard, both holders: up refuses what is already up ───────────────────────────────────

prova.test("up refuses a topology a detached holder already has, and teaches both exits", {
  covers = "docs/design/agent-ergonomics.md#mcp-up-does-not-see-a-detached-hold",
  proves = "the warm `up` guarded only against its OWN registry, so standing up a name a terminal `prova up`/`prova start` already held provisioned a SECOND instance — minutes of work and a port collision for anything on fixed host ports, silent because nothing in either holder could see the other",
}, function(t)
  local root = t:use(scratch)()
  registered(root)
  local count = root .. "/count.txt"

  local started = shell.run(prova.bin .. " start orders",
    { cwd = root, env = { PROVA_PROOF_COUNT = count }, merge_stderr = true })
  t:defer(function() shell.run(prova.bin .. " down orders", { cwd = root, merge_stderr = true }) end)
  t:expect(started.code, "the detached holder came up"):equals(0)
  t:expect(fs.read(count), "and provisioned once"):equals("1")

  local by_id = mcp(t, root, {
    call(2, "up", { name = "orders" }),
    call(3, "status"),
  }, { PROVA_PROOF_COUNT = count })

  t:expect(by_id[2].result.isError, "the second stand-up is refused"):is_truthy()
  local msg = by_id[2].result.content[1].text
  t:expect(msg, "says what stopped it"):contains("already up")
  t:expect(msg, "names the holder"):contains("pid " .. json.decode(
    fs.read(root .. "/.prova/var/running/orders.json")).pid)
  -- Two exits, and neither is `down` on this server: the holder is not the server's to reap.
  t:expect(msg, "teaches the reap"):contains("prova down orders")
  t:expect(msg, "teaches the attach"):contains("--topology orders")
  -- The load-bearing negative: a refusal that still provisioned would pass every assertion above.
  t:expect(fs.read(count), "nothing was provisioned a second time"):equals("1")

  -- And the refusal is not a blind one — the same server can see exactly what stopped it.
  local held = tool_json(by_id[3]).held
  t:expect(#held, "the detached hold, from the server that was refused"):equals(1)
  t:expect(held[1].holder):equals("detached")
end)

prova.test("a STALE record does not block a warm up — the dead holder's litter is cleared", {
  covers = "docs/design/agent-ergonomics.md#mcp-up-does-not-see-a-detached-hold",
  proves = "the guard must refuse a LIVE holder, not the file it leaves behind: a record whose process is gone is not a hold, and treating it as one would make an ungraceful teardown anywhere in the package's history permanently un-`up`-able over MCP",
}, function(t)
  local root = t:use(scratch)()
  registered(root)
  local count = root .. "/count.txt"

  -- A holder that died without cleaning up, spelled as what it leaves on disk. The pid is the
  -- one `runstate`'s own tests use for "almost certainly not alive". `endpoints` goes through
  -- `json.decode("[]")` rather than `{}` because a bare empty Lua table encodes as an object and
  -- the record would not parse at all ([[a-list-verb-returns-a-list]]) — which would make this
  -- proof pass for the wrong reason: an unreadable record is skipped, not cleared.
  fs.mkdir(root .. "/.prova/var/running")
  fs.write(root .. "/.prova/var/running/orders.json", json.encode({
    name = "orders", pid = 999999999, started_at = 1,
    endpoints = json.decode("[]"), value = {},
  }))

  local by_id = mcp(t, root, {
    call(2, "up", { name = "orders" }),
  }, { PROVA_PROOF_COUNT = count })

  -- Read the error flag BEFORE decoding: a refusal's content is prose, so decoding first turns a
  -- real regression into a json.decode traceback instead of naming what went wrong.
  t:expect(by_id[2].result.isError, "a dead holder is not a hold: " ..
    by_id[2].result.content[1].text):never():is_truthy()
  t:expect(tool_json(by_id[2]).name):equals("orders")
  t:expect(fs.read(count), "it provisioned, exactly once"):equals("1")
  t:expect(fs.exists(root .. "/.prova/var/running/orders.json"),
    "and the dead holder's record was cleared, not inherited"):equals(false)
end)

-- ── warm re-run: resolve, never provision ────────────────────────────────────────────────────

prova.test("run{topology} resolves the held instance: one provision, state accumulating across runs",
  { covers = "docs/design/mcp-mode.md#warm-rerun-held-injection" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local count, hits, marker = root .. "/count.txt", root .. "/hits.txt", root .. "/marker.txt"

  local by_id = mcp(t, root, {
    call(2, "up", { name = "orders" }),
    call(3, "run", { topology = "orders" }),
    call(4, "run", { topology = "orders" }),
  }, { PROVA_PROOF_COUNT = count, PROVA_PROOF_HITS = hits, PROVA_PROOF_MARKER = marker })

  local _, up_err = tool_json(by_id[2])
  t:expect(up_err, "up succeeded"):never():is_truthy()
  for id = 3, 4 do
    local run = tool_json(by_id[id])
    t:expect(run.passed, "warm run " .. id .. " passed"):equals(1)
    t:expect(run.failed):equals(0)
  end
  t:expect(fs.read(count), "the factory ran exactly once"):equals("1")
  t:expect(fs.read(hits), "the second run saw the first run's mutation"):equals("2")
  -- Server shutdown (stdin EOF) reaps what is still held — the holder is the only reaper.
  t:expect(fs.read(marker)):equals("torn-down")
end)

prova.test("run{topology} without a held environment is an explicit error, not an implicit up",
  { covers = "docs/design/mcp-mode.md#warm-rerun-held-injection" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local by_id = mcp(t, root, { call(2, "run", { topology = "orders" }) })
  t:expect(by_id[2].result.isError):is_truthy()
  t:expect(by_id[2].result.content[1].text):contains("not held")
end)

-- ── eval: one execution path, both transports ────────────────────────────────────────────────

prova.test("eval runs one-shot code in the full environment on both transports",
  { covers = "docs/design/mcp-mode.md#eval-one-shot" }, function(t)
  local root = t:use(scratch)()
  registered(root)

  local cli = shell.run(prova.bin .. " eval 'return 21 * 2'", { cwd = root, merge_stderr = true })
  t:expect(cli.code):equals(0)
  t:expect(cli.stdout):contains("42")

  local by_id = mcp(t, root, { call(2, "eval", { code = "return json.encode({ok = true})" }) })
  local value, is_err = tool_json(by_id[2])
  t:expect(is_err):never():is_truthy()
  t:expect(value, "modules are available, same as the CLI"):contains("ok")
end)

-- ── the reminders lane, narrowed the same way on both surfaces ───────────────────────────────

--- A package with one due and one watching reminder, its record populated by a CLI run — the
--- MCP tool reads the same record the CLI verb reads.
local function reminded(root)
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/watch_test.lua", [[
prova.test("green", function(t) t:expect(true):is_true() end)
prova.remind("cert-expiry", { when = function() return "expired" end, tags = { "ops" } }, "rotate")
prova.remind("deps-current", { when = function() return false end, tags = { "deps" } }, "bump")
]])
  shell.run(prova.bin, { cwd = root, merge_stderr = true })
end

prova.test("the reminders tool narrows by state, and isError answers only for what is listed",
  { covers = "docs/design/reminders.md#reminders-state-filters" }, function(t)
  local root = t:use(scratch)()
  reminded(root)
  local by_id = mcp(t, root, {
    call(2, "reminders"),
    call(3, "reminders", { state = "watching" }),
    call(4, "reminders", { state = "due" }),
    call(5, "reminders", { state = "snoozed" }),
  })
  local full, full_err = tool_json(by_id[2])
  t:expect(#full.reminders):equals(2)
  t:expect(full_err, "a due reminder marks the full report"):is_truthy()
  local watching, watching_err = tool_json(by_id[3])
  t:expect(#watching.reminders):equals(1)
  t:expect(watching.reminders[1].name):equals("deps-current")
  t:expect(watching_err, "the narrowed report answers only for what it lists"):never():is_truthy()
  local due, due_err = tool_json(by_id[4])
  t:expect(#due.reminders):equals(1)
  t:expect(due.reminders[1].name):equals("cert-expiry")
  t:expect(due_err):is_truthy()
  t:expect(by_id[5].result.isError, "an unknown state is an error, not an empty list"):is_truthy()
  t:expect(by_id[5].result.content[1].text):contains("due")
end)

prova.test("the reminders tool takes the same selector axes the CLI verb takes",
  { covers = "docs/design/reminders.md#reminders-selectors-narrow" }, function(t)
  local root = t:use(scratch)()
  reminded(root)
  local by_id = mcp(t, root, {
    call(2, "reminders", { keywords = { "deps" } }),
    call(3, "reminders", { tags = { "ops" } }),
    call(4, "reminders", { nodes = { "deps-current" } }),
    call(5, "reminders", { keyword_excludes = { "deps" } }),
  })
  local k = tool_json(by_id[2])
  t:expect(#k.reminders):equals(1)
  t:expect(k.reminders[1].name):equals("deps-current")
  local tags = tool_json(by_id[3])
  t:expect(#tags.reminders):equals(1)
  t:expect(tags.reminders[1].name):equals("cert-expiry")
  local node = tool_json(by_id[4])
  t:expect(#node.reminders):equals(1)
  t:expect(node.reminders[1].name):equals("deps-current")
  local excl = tool_json(by_id[5])
  t:expect(#excl.reminders):equals(1)
  t:expect(excl.reminders[1].name):equals("cert-expiry")
end)

-- ── the selection axes: one grammar, both surfaces ───────────────────────────────────────────

prova.test("every selection axis narrows the tests lane identically on both surfaces",
  { covers = "docs/design/mcp-mode.md#selection-axes-parity" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/axes_test.lua", [[
prova.test("alpha api", { tags = { "api" } }, function(t) t:expect(true):is_true() end)
prova.test("beta ops", { tags = { "ops" } }, function(t) t:expect(true):is_true() end)
]])

  local function cli(flags)
    local r = shell.run(prova.bin .. " --list " .. flags, { cwd = root })
    local out = {}
    for line in r.stdout:gmatch("[^\n]+") do out[#out + 1] = line end
    table.sort(out)
    return out
  end
  local full = cli("--allow-empty")
  t:expect(#full):equals(2)
  local alpha_node
  for _, p in ipairs(full) do
    if p:find("alpha", 1, true) then alpha_node = p end
  end

  local by_id = mcp(t, root, {
    call(2, "tests", { keywords = { "alpha" } }),
    call(3, "tests", { keyword_excludes = { "alpha" } }),
    call(4, "tests", { tags = { "ops" } }),
    call(5, "tests", { tag_excludes = { "ops" } }),
    call(6, "tests", { nodes = { alpha_node } }),
  })
  local function tool_paths(id)
    local out = {}
    for _, n in ipairs(tool_json(by_id[id]).nodes) do out[#out + 1] = n.path end
    table.sort(out)
    return out
  end
  -- Each CLI narrowing must actually narrow (1 of 2) — otherwise an axis BOTH surfaces dropped
  -- would compare equal vacuously — and the MCP spelling must select the same set.
  local cases = {
    { id = 2, flags = "-k alpha", axis = "keywords" },
    { id = 3, flags = "-k '!alpha'", axis = "keyword_excludes" },
    { id = 4, flags = "--tags ops", axis = "tags" },
    { id = 5, flags = "--tags '!ops'", axis = "tag_excludes" },
    { id = 6, flags = '--node "' .. alpha_node .. '"', axis = "nodes" },
  }
  for _, case in ipairs(cases) do
    local narrowed = cli(case.flags)
    t:expect(#narrowed, case.axis .. " narrows on the CLI"):equals(1)
    t:expect(json.encode(tool_paths(case.id)), case.axis .. " selects the same set over MCP")
      :equals(json.encode(narrowed))
  end
end)

prova.test("the run tool throws switches — the MCP door to the opt-in classes",
  { covers = "docs/design/manifest.md#switches-not-env-capabilities" }, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(root .. "/proofs/gated_test.lua", [[
prova.test("ordinary", function(t) t:expect(true):is_true() end)
prova.test("heavy", { switch = "heavy" }, function(t) t:expect(true):is_true() end)
]])
  local by_id = mcp(t, root, {
    call(2, "run"),
    call(3, "run", { switches = { "heavy" } }),
  })
  local bare = tool_json(by_id[2])
  t:expect(bare.passed, "the class is off unless thrown, over MCP exactly as on the CLI"):equals(1)
  local thrown = tool_json(by_id[3])
  t:expect(thrown.passed):equals(2)
end)

-- ── one skill, three doors ───────────────────────────────────────────────────────────────────

prova.test("the one embedded skill: printed, served as instructions, and installed",
  { covers = "docs/design/mcp-mode.md#skill-embedded-everywhere" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local fingerprint = "Proof-Driven Development"

  local printed = shell.run(prova.bin .. " skill", { cwd = root })
  t:expect(printed.code):equals(0)
  t:expect(printed.stdout):contains(fingerprint)

  local by_id = mcp(t, root, {})
  t:expect(by_id[1].result.instructions, "served on connect"):contains(fingerprint)

  local installed = shell.run(prova.bin .. " skill --install", { cwd = root, merge_stderr = true })
  t:expect(installed.code):equals(0)
  t:expect(root .. "/.claude/skills/prova/SKILL.md"):exists()
  t:expect(fs.read(root .. "/.claude/skills/prova/SKILL.md")):contains(fingerprint)
end)

-- ── capture: the specs lane's one write, verified ────────────────────────────────────────────

prova.test("the capture tool writes a scanned anchor, stamps the date, and refuses the unscannable", {
  covers = "docs/design/mcp-mode.md#backlog-capture-is-a-taught-procedure",
  proves = "an agent told to capture something used to guess at a file, and a plausible guess (a plan, a README) landed the item where the ledger never scans — capture that silently did not capture",
}, function(t)
  local root = t:use(scratch)()
  fs.mkdir(root .. "/docs")
  fs.mkdir(root .. "/proofs")
  fs.write(root .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(root .. "/docs/design.md",
    "# design\n\n<!-- claim: existing-item -->\nAlready anchored, for the duplicate-id refusal.\n")

  local by_id = mcp(t, root, {
    -- A good capture: under the source, fresh id — lands, dated, addressable.
    call(2, "capture", { state = "backlog", id = "lease-renewal", prose = "Leases should renew before expiry.", file = "docs/design.md" }),
    -- A plausible-but-unscanned path: refused, naming the sources.
    call(3, "capture", { state = "backlog", id = "lost-item", prose = "x", file = "README.md" }),
    -- A duplicate id: refused, naming the existing address.
    call(4, "capture", { state = "claim", id = "existing-item", prose = "x", file = "docs/design.md" }),
    -- The lane sees the capture (the same server, same scan the ledger uses).
    call(5, "specs", { state = "backlog" }),
  })

  local captured, err = tool_json(by_id[2])
  t:expect(err, "the capture succeeded"):never():is_truthy()
  t:expect(captured.captured.address):equals("docs/design.md#lease-renewal")
  local doc = fs.read(root .. "/docs/design.md")
  t:expect(doc, "the capture stamp is the anchor's blessed property"):contains("<!-- backlog: lease-renewal recorded=20")

  t:expect(by_id[3].result.isError, "an unscanned path is refused"):is_truthy()
  t:expect(by_id[3].result.content[1].text, "the refusal names the sources"):contains("docs")

  t:expect(by_id[4].result.isError, "a duplicate id is refused"):is_truthy()
  t:expect(by_id[4].result.content[1].text, "the refusal names the existing address")
    :contains("docs/design.md#existing-item")

  local lane = tool_json(by_id[5])
  local seen = false
  for _, row in ipairs(lane.specs) do
    if row.address == "docs/design.md#lease-renewal" then seen = true end
  end
  t:expect(seen, "the lane scans what capture wrote"):is_true()
end)
