--- THE PROOF FOR `prova mcp` — written before the implementation existed (Proof-Driven
--- Development, applied to Prova itself). Black-box: spawn the real binary in MCP stdio mode,
--- speak newline-delimited JSON-RPC in a batch (requests in, EOF, responses out), and hold the
--- server to its contract:
---
---   * initialize returns serverInfo.name "prova" and the embedded agent skill as `instructions`
---   * tools/list exposes exactly the CLI-parity cold tools: run, list, eval
---   * every tool returns ONE text content item whose text is JSON (the stable machine contract)
---   * `list` honors selection; `run` returns counts + per-failure detail; `eval` evaluates in
---     the full environment; the server exits 0 on stdin EOF
---
--- The launcher (tests/selftest.rs) sets PROVA_BIN and PROVA_FIXTURES.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")
local fixtures = assert(os.getenv("PROVA_FIXTURES"), "PROVA_FIXTURES not set")
local project = fixtures .. "/mcp-project"

-- Send a batch of JSON-RPC messages to `prova mcp` over stdio; return { responses_by_id, result }.
-- MCP stdio framing is newline-delimited JSON. The batch always opens with the handshake.
local function mcp(messages)
  local batch = {
    { jsonrpc = "2.0", id = 1, method = "initialize", params = {
        protocolVersion = "2024-11-05",
        capabilities = {},
        clientInfo = { name = "prova-selftest", version = "0" },
      } },
    { jsonrpc = "2.0", method = "notifications/initialized" },
  }
  for _, m in ipairs(messages) do batch[#batch + 1] = m end

  local dir = fs.tempdir()
  local req = dir .. "/requests.jsonl"
  local lines = {}
  for _, m in ipairs(batch) do lines[#lines + 1] = json_encode(m) end
  fs.write(req, table.concat(lines, "\n") .. "\n")

  local r = shell.run(prova_bin .. " mcp < " .. req, { cwd = project, timeout = "60s" })
  local by_id = {}
  for _, line in ipairs(prova.parse.lines(r.stdout)) do
    local ok, msg = pcall(prova.parse.json, line)
    if ok and type(msg) == "table" and msg.id ~= nil then by_id[msg.id] = msg end
  end
  return by_id, r
end

-- Minimal JSON encoder for request batches (strings/numbers/bools/tables; enough for JSON-RPC —
-- request strings here never contain newlines or exotic escapes).
function json_encode(v)
  local t = type(v)
  if t == "string" then
    return '"' .. v:gsub('\\', '\\\\'):gsub('"', '\\"') .. '"' 
  elseif t == "number" or t == "boolean" then
    return tostring(v)
  elseif t == "table" then
    local is_array = #v > 0 or next(v) == nil
    local parts = {}
    if is_array and next(v) ~= nil then
      for _, item in ipairs(v) do parts[#parts + 1] = json_encode(item) end
      return "[" .. table.concat(parts, ",") .. "]"
    elseif next(v) == nil then
      return "{}"
    else
      for k, item in pairs(v) do
        parts[#parts + 1] = string.format("%q", k) .. ":" .. json_encode(item)
      end
      return "{" .. table.concat(parts, ",") .. "}"
    end
  end
  error("unencodable type: " .. t)
end

-- Every tool result is one text content item whose text is JSON — decode it.
local function tool_json(response, label)
  assert(response, (label or "tool") .. ": no response")
  assert(response.result, (label or "tool") .. ": error: " .. json_encode(response.error or {}))
  local content = response.result.content
  assert(type(content) == "table" and content[1] and content[1].type == "text",
    (label or "tool") .. ": expected one text content item")
  return prova.parse.json(content[1].text), response.result.isError
end

prova.group("prova mcp", function(g)
  g:test("initialize: serverInfo + the skill as instructions; clean exit on EOF", function(t)
    local by_id, r = mcp({})
    t:expect(r.code, "server exits 0 on stdin EOF"):equals(0)
    local init = by_id[1]
    t:expect(init and init.result and init.result.serverInfo.name):equals("prova")
    t:expect(init.result.instructions):contains("Proof-Driven Development")
    t:expect(init.result.instructions):contains("prova --last-failed")
  end)

  g:test("tools/list exposes the CLI-parity cold tools", function(t)
    local by_id = mcp({ { jsonrpc = "2.0", id = 2, method = "tools/list" } })
    local tools = assert(by_id[2] and by_id[2].result and by_id[2].result.tools, "no tools result")
    local names = {}
    for _, tool in ipairs(tools) do names[tool.name] = true end
    t:expect_all(function()
      t:expect(names.run, "run tool"):is_true()
      t:expect(names.list, "list tool"):is_true()
      t:expect(names.eval, "eval tool"):is_true()
    end)
  end)

  g:test("eval evaluates in the full environment and returns JSON", function(t)
    local by_id = mcp({
      { jsonrpc = "2.0", id = 3, method = "tools/call",
        params = { name = "eval", arguments = { code = "return 21 * 2" } } },
    })
    local value = tool_json(by_id[3], "eval")
    t:expect(value):equals(42)
  end)

  g:test("list discovers the project's nodes and honors selection", function(t)
    local by_id = mcp({
      { jsonrpc = "2.0", id = 4, method = "tools/call",
        params = { name = "list", arguments = {} } },
      { jsonrpc = "2.0", id = 5, method = "tools/call",
        params = { name = "list", arguments = { tags = { "slow" } } } },
    })
    local all = tool_json(by_id[4], "list")
    t:expect(#all.nodes):equals(3)
    local slow = tool_json(by_id[5], "list selected")
    t:expect(#slow.nodes):equals(1)
    t:expect(slow.nodes[1].path):contains("tagged slow")
  end)

  g:test("run returns counts and per-failure detail; selection deselects", function(t)
    local by_id = mcp({
      { jsonrpc = "2.0", id = 6, method = "tools/call",
        params = { name = "run", arguments = {} } },
      { jsonrpc = "2.0", id = 7, method = "tools/call",
        params = { name = "run", arguments = { keywords = { "always passes" } } } },
    })
    local full, full_err = tool_json(by_id[6], "run")
    t:expect(full.passed):equals(2)
    t:expect(full.failed):equals(1)
    t:expect(full_err, "a failing run marks isError"):is_true()
    local failure = assert(full.failures and full.failures[1], "run result carries failures[]")
    t:expect(failure.path):contains("always fails")
    t:expect(failure.message):contains("deliberate red")

    local narrow = tool_json(by_id[7], "run selected")
    t:expect(narrow.passed):equals(1)
    t:expect(narrow.failed):equals(0)
    t:expect(narrow.deselected):equals(2)
  end)
end)
