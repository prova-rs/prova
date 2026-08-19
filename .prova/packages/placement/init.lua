-- Test-support client for the placement conformance suite (docs/design/placement.md).
--
-- Deliberately thin and deliberately dumb: it frames requests and decodes responses and does
-- NOTHING else. No retry, no outcome interpretation, no defaults filled in. The suite is proving a
-- broker's behaviour, so every rule under proof has to be visible in the test body rather than
-- smuggled into this helper — a client that quietly retried a `busy` would make the busy/skip
-- distinction untestable, which is the most important rule in the spec.
--
-- This is a test client, not prova's placement client. The real one lives in prova-core; this
-- exists so the conformance suite can speak the protocol from Lua, at arm's length from whatever
-- the engine does.

local M = {}

--- The externally configured broker address, or nil when none is named.
---@return string|nil
function M.address()
	return os.getenv("PROVA_PLACEMENT_BROKER")
end

--- The broker under proof: the configured external one, or the MIT reference broker spawned
--- fresh for this test. Self-spawning is what makes the suite hermetic — the spec's proofs run
--- on any unix machine with no setup — while the env var keeps the same suite pointable at any
--- third-party broker, which is how one proves itself.
---@param ctx any
---@return string address
function M.broker(ctx)
	local addr = M.address()
	if addr then
		return addr
	end
	-- A SHORT socket path, deliberately: sockaddr_un caps the path near 104 bytes, and a tempdir
	-- path plus a filename can exceed it. /tmp plus eight hex characters cannot.
	local sock = "/tmp/prova-b-" .. uuid.v4():sub(1, 8) .. ".sock"
	local proc = shell.spawn({
		prova.bin, "broker", "--socket", sock, "--offer", "prova-conformance-slot",
	})
	ctx:defer(function()
		pcall(function()
			proc:stop()
		end)
		pcall(function()
			fs.remove_all(sock)
		end)
	end)
	-- Readiness is a CONNECTION, not a file. The socket file appears at bind(), a hair before
	-- listen() — and on a noisy runner the scheduler can widen that hair into a refused connect
	-- (it did, once, on macOS CI). Probing with a real connect closes both races: file-not-yet
	-- and listening-not-yet read as the same "not ready, ask again".
	local spawned = "unix://" .. sock
	prova.retry(function()
		local ok, probe = pcall(socket.connect, ctx, { addr = spawned, framing = { delimiter = "\n" } })
		if not ok then
			return false
		end
		pcall(function()
			probe:close()
		end)
		return true
	end, {
		timeout = "10s",
		every = "25ms",
		message = "the reference broker never came up on " .. spawned,
	})
	return spawned
end

--- The protocol version this suite speaks. Bump with the spec, never silently.
M.PROTOCOL = "1.0"

--- Dial the broker. Newline-delimited JSON, one frame per turn.
---
--- Connections are managed by the scope, so a spec never leaks a socket into the next test — which
--- matters here because leases are keyed to connections in some broker designs. That is now the
--- DRIVER's promise rather than this package's: `socket.connect` takes the context and closes with
--- the scope, so the hand-rolled defer that used to live here is gone.
---@param ctx any
---@param addr string|nil defaults to `M.broker(ctx)` — the external broker, or a spawned reference one
function M.connect(ctx, addr)
	local conn = socket.connect(ctx, {
		addr = addr or M.broker(ctx),
		framing = { delimiter = "\n" },
	})

	local next_id = 0

	local client = {}

	--- Send one request and read one terminal frame. Raises on a timeout or a closed connection.
	---
	--- `event` frames sharing the id are collected and returned alongside, so a streaming op reads
	--- the same as a plain one at the call site.
	---@param op string
	---@param fields table|nil
	---@return table response, table events
	function client:request(op, fields)
		next_id = next_id + 1
		local frame = { id = next_id, op = op }
		for k, v in pairs(fields or {}) do
			frame[k] = v
		end
		conn:send(json.encode(frame))

		local events = {}
		while true do
			local decoded = json.decode(conn:recv())
			if decoded.event then
				events[#events + 1] = decoded
			else
				return decoded, events
			end
		end
	end

	--- Send a raw frame verbatim — for proving what a broker does with a malformed or
	--- out-of-order one, which a typed helper would make impossible to express.
	---@param raw string
	---@return table
	function client:send_raw(raw)
		conn:send(raw)
		return json.decode(conn:recv())
	end

	--- The mandatory opening turn. Returns the broker's response so a spec can assert on the
	--- negotiated version and advertised features.
	---@return table
	function client:hello()
		local response = self:request("hello", {
			protocol = M.PROTOCOL,
			client = "prova-conformance",
			run = "R-conformance",
		})
		return response
	end

	return client
end

--- True when the broker advertised an optional plane in its hello response. A spec must not send
--- an op the broker did not advertise, so this is how the optional planes gate themselves.
---@param response table
---@param feature string
---@return boolean
function M.advertises(response, feature)
	for _, name in ipairs(response.features or {}) do
		if name == feature then
			return true
		end
	end
	return false
end

return M
