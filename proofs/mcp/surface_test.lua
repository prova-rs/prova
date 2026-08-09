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
  return function() return ctx:tempdir() end
end)

--- Drive `prova mcp` with one batch: handshake + `lines` (raw JSON-RPC strings), stdin EOF,
--- responses decoded by id. One process per call — ordering across tool calls IS the warmth.
local function mcp(dir, lines, env)
  local req = dir .. "/requests.jsonl"
  local all = {
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05",'
      .. '"capabilities":{},"clientInfo":{"name":"proof","version":"0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
  }
  for _, l in ipairs(lines) do all[#all + 1] = l end
  fs.write(req, table.concat(all, "\n") .. "\n")
  local r = shell.run(prova.bin .. " mcp < " .. req, { cwd = dir, env = env or {}, timeout = "60s" })
  local by_id = {}
  for line in r.stdout:gmatch("[^\n]+") do
    local ok, msg = pcall(json.decode, line)
    if ok and type(msg) == "table" and msg.id then by_id[msg.id] = msg end
  end
  return by_id, r
end

--- A tool result's decoded JSON payload plus its error flag.
local function tool_json(resp)
  return json.decode(resp.result.content[1].text), resp.result.isError
end

local function call(id, tool, arguments_json)
  return '{"jsonrpc":"2.0","id":' .. id .. ',"method":"tools/call","params":{"name":"' .. tool
    .. '","arguments":' .. (arguments_json or "{}") .. "}}"
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

-- ── the parity contract ──────────────────────────────────────────────────────────────────────

prova.test("the tool surface mirrors the CLI verbs, warm holder included",
  { covers = "docs/design/mcp-mode.md#mcp-cli-parity" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local by_id = mcp(root, { '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' })
  local names = {}
  for _, tool in ipairs(by_id[2].result.tools) do names[tool.name] = true end
  -- The FULL surface, not a sample: a tool silently dropped from the server would slip past a
  -- partial list (the gap query-consolidation increment 1 flagged). Lanes + account + drivers +
  -- the warm holder. `tests` is the tests lane (formerly `list`); `status` is the held-registry
  -- view (increment 7 reconciles its name with the CLI's `ps`).
  for _, expected in ipairs({
    "run", "tests", "specs", "reminders", "eval", "learn", "introspect", "capabilities",
    "packages", "attest", "evidence", "owed", "up", "down", "status",
  }) do
    t:expect(names[expected], "tool " .. expected .. " is served"):is_truthy()
  end
end)

-- ── the held registry: status, never ps ──────────────────────────────────────────────────────

prova.test("a server-held topology reports through status and never writes a running record",
  { covers = "docs/design/mcp-mode.md#held-visible-via-status-not-ps" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  -- Hold it and ask status; no `down`, so the hold spans the batch. `prova ps` reads the
  -- `running/` records that `prova start` writes — a server-held topology must never mint one,
  -- which is exactly why it is invisible to `ps` and visible to `status`.
  local by_id = mcp(root, {
    call(2, "up", '{"name":"orders"}'),
    call(3, "status"),
  })
  local held = tool_json(by_id[3]).held
  t:expect(held[1].name):equals("orders")
  t:expect(held[1].resources[1].url):equals("http://127.0.0.1:19999")
  t:expect(fs.exists(root .. "/.prova/var/running/orders.json"),
    "no detached record was ever written"):equals(false)
end)

-- ── warm re-run: resolve, never provision ────────────────────────────────────────────────────

prova.test("run{topology} resolves the held instance: one provision, state accumulating across runs",
  { covers = "docs/design/mcp-mode.md#warm-rerun-held-injection" }, function(t)
  local root = t:use(scratch)()
  registered(root)
  local count, hits, marker = root .. "/count.txt", root .. "/hits.txt", root .. "/marker.txt"

  local by_id = mcp(root, {
    call(2, "up", '{"name":"orders"}'),
    call(3, "run", '{"topology":"orders"}'),
    call(4, "run", '{"topology":"orders"}'),
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
  local by_id = mcp(root, { call(2, "run", '{"topology":"orders"}') })
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

  local by_id = mcp(root, { call(2, "eval", '{"code":"return json.encode({ok = true})"}') })
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
  local by_id = mcp(root, {
    call(2, "reminders"),
    call(3, "reminders", '{"state":"watching"}'),
    call(4, "reminders", '{"state":"due"}'),
    call(5, "reminders", '{"state":"snoozed"}'),
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
  local by_id = mcp(root, {
    call(2, "reminders", '{"keywords":["deps"]}'),
    call(3, "reminders", '{"tags":["ops"]}'),
    call(4, "reminders", '{"nodes":["deps-current"]}'),
    call(5, "reminders", '{"keyword_excludes":["deps"]}'),
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

  local by_id = mcp(root, {
    call(2, "tests", '{"keywords":["alpha"]}'),
    call(3, "tests", '{"keyword_excludes":["alpha"]}'),
    call(4, "tests", '{"tags":["ops"]}'),
    call(5, "tests", '{"tag_excludes":["ops"]}'),
    call(6, "tests", json.encode({ nodes = { alpha_node } })),
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
  local by_id = mcp(root, {
    call(2, "run"),
    call(3, "run", '{"switches":["heavy"]}'),
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

  local by_id = mcp(root, {})
  t:expect(by_id[1].result.instructions, "served on connect"):contains(fingerprint)

  local installed = shell.run(prova.bin .. " skill --install", { cwd = root, merge_stderr = true })
  t:expect(installed.code):equals(0)
  t:expect(root .. "/.claude/skills/prova/SKILL.md"):exists()
  t:expect(fs.read(root .. "/.claude/skills/prova/SKILL.md")):contains(fingerprint)
end)
