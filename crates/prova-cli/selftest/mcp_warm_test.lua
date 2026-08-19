--- THE PROOF FOR THE WARM PHASE — written before the implementation existed. The MCP server is a
--- topology holder: `up` provisions a named topology ONCE inside the server; `run { topology }`
--- resolves the HELD instance instead of re-provisioning (the same Lua values, warm across tool
--- calls); `eval { topology }` exposes the held value as a global named after the topology;
--- `status` lists what's held; `down` runs the one true teardown. Observables: the warm-project
--- fixture writes `provisions` / `hits` / `teardown` sentinel files, and its held value carries a
--- mutable counter table — re-provisioning would reset it, only true warmth accumulates it.
---
--- The binary under test is `prova.bin` (the runtime injects it); the launcher
--- (tests/selftest.rs) sets PROVA_FIXTURES.

local prova_bin = assert(prova.bin, "prova.bin not injected by the runtime")
local fixtures = assert(os.getenv("PROVA_FIXTURES"), "PROVA_FIXTURES not set")
local project = fixtures .. "/warm-project"


-- One server process, one batch: the ordering across tool calls IS what warmth means.
--- Drive `prova mcp` as a real CONVERSATION over `stdio.spawn`: each request written only after
--- the previous reply has come back, so the ordering these proofs assert is ORDERING rather than
--- an assumption about how the server schedules.
---
--- This was a batch — every message into a `requests.jsonl`, redirected in — which worked only
--- because prova's server answers sequentially; against one free to dispatch concurrently the
--- same batch answers turn two before turn one has stored anything
--- (`agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut`). The hand-rolled `json_encode`
--- went with it: `codec = "json"` encodes the turn.
local function mcp(t, messages)
  local sess = stdio.spawn(t, {
    cmd = { prova_bin, "mcp" },
    cwd = project,
    framing = "line",     -- MCP stdio framing is newline-delimited JSON
    codec = "json",
  })
  sess:send({ jsonrpc = "2.0", id = 1, method = "initialize", params = {
    protocolVersion = "2024-11-05",
    capabilities = {},
    clientInfo = { name = "prova-selftest", version = "0" },
  } })
  local by_id = { [1] = sess:recv({ where = { id = 1 }, timeout = "60s" }) }
  sess:send({ jsonrpc = "2.0", method = "notifications/initialized" })

  for _, m in ipairs(messages) do
    sess:send(m)
    if m.id ~= nil then
      by_id[m.id] = sess:recv({ where = { id = m.id }, timeout = "120s" })
    end
  end
  sess:eof()
  return by_id, { code = sess:wait({ timeout = "60s" }) }
end

local function tool_json(response, label)
  assert(response, (label or "tool") .. ": no response")
  assert(response.result, (label or "tool") .. ": rpc error: " .. json.encode(response.error or {}))
  local content = response.result.content
  assert(type(content) == "table" and content[1] and content[1].type == "text",
    (label or "tool") .. ": expected one text content item")
  return json.decode(content[1].text), response.result.isError
end

local function call(id, name, arguments)
  return { jsonrpc = "2.0", id = id, method = "tools/call",
           params = { name = name, arguments = arguments or {} } }
end

-- Fresh sentinels per proof run.
local function clean()
  for _, f in ipairs({ "provisions", "hits", "teardown" }) do
    if fs.exists(project .. "/" .. f) then fs.remove_all(project .. "/" .. f) end
  end
end

prova.group("prova mcp — warm phase", function(g)
  g:test("the full warm lifecycle: up once, run twice warm, eval sees state, down tears down", function(t)
    clean()
    local by_id = mcp(t, {
      call(2, "up", { name = "warmtop" }),
      call(3, "status"),
      call(4, "run", { topology = "warmtop" }),
      call(5, "run", { topology = "warmtop" }),
      call(6, "eval", { code = "return warmtop.counter.hits", topology = "warmtop" }),
      call(7, "down", { name = "warmtop" }),
      call(8, "status"),
    })

    -- up: provisions exactly once, reports the resource url.
    local up = tool_json(by_id[2], "up")
    t:expect(up.name):equals("warmtop")
    t:expect(json.encode(up)):contains("mem://warmtop")
    t:expect(fs.read(project .. "/provisions")):equals("1")

    -- status while held: names the topology.
    local held = tool_json(by_id[3], "status")
    t:expect(json.encode(held)):contains("warmtop")

    -- two warm runs: both green, NO re-provisioning, and the SAME held Lua table accumulates.
    local run1 = tool_json(by_id[4], "warm run 1")
    t:expect(run1.passed):equals(1)
    local run2 = tool_json(by_id[5], "warm run 2")
    t:expect(run2.passed):equals(1)
    t:expect(fs.read(project .. "/provisions"), "provisioned exactly once"):equals("1")
    t:expect(fs.read(project .. "/hits"), "held state accumulated across runs"):equals("2")

    -- eval inside the held env: the topology's value is a global named after it.
    local hits = tool_json(by_id[6], "warm eval")
    t:expect(hits):equals(2)

    -- down: the one true teardown ran; nothing held afterwards.
    tool_json(by_id[7], "down")
    t:expect(fs.exists(project .. "/teardown"), "teardown sentinel written"):is_true()
    local after = tool_json(by_id[8], "status after down")
    t:expect(json.encode(after)):never():contains("warmtop")
    clean()
  end)

  g:test("run{topology} without a held environment is an explicit error", function(t)
    clean()
    local by_id = mcp(t, { call(2, "run", { topology = "warmtop" }) })
    local resp = assert(by_id[2], "no response")
    local is_error = (resp.result and resp.result.isError) or (resp.error ~= nil)
    t:expect(is_error, "not-held must be an explicit error"):is_true()
    t:expect(fs.exists(project .. "/provisions"), "must not silently cold-provision"):is_false()
    clean()
  end)
end)
