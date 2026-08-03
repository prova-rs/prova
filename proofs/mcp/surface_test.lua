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

[plugins]
kitchen = "plugins/kitchen.lua"

[topologies]
orders = { plugin = "kitchen", factory = "orders" }
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
  for _, expected in ipairs({ "run", "list", "eval", "up", "down", "status", "learn", "introspect" }) do
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
