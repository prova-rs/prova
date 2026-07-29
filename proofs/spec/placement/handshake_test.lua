-- The opening turn and the framing beneath it (docs/design/placement.md §Transport, §Frames).
--
-- Nothing else in the suite can be trusted until this is nailed down: every later proof depends on
-- knowing which protocol version the broker agreed to speak and which optional planes it claims.

local placement = require("placement")

-- One options table, shared by every proof in this file: open until a broker exists, and gated on
-- an address having been named. `requires` skips where there is nothing to prove against; the spec
-- flag keeps it open until something can pass.
local OPEN = {
	spec = "placement: no broker implementation yet (docs/design/placement.md)",
	requires = { "placement_broker" },
}

prova.test("hello negotiates a version the broker will actually speak", OPEN, function(t)
	local broker = placement.connect(t)
	local response = broker:hello()

	t:expect(response.ok, "the handshake succeeded"):is_true()
	-- The version the broker ECHOES is the contract, not the one the client asked for. A broker
	-- that stays silent here leaves the client guessing, and a client that assumes its own version
	-- was accepted will send fields the broker never agreed to parse.
	t:expect(response.protocol, "the negotiated version is stated"):matches("^1%.")
	t:expect(response.broker, "the broker identifies itself"):never():is_nil()
end)

prova.test("the broker refuses any operation before hello", OPEN, function(t)
	local broker = placement.connect(t)
	local response = broker:request("resolve", { capabilities = { { name = "sh" } } })

	-- A client that skips the handshake has almost certainly failed to negotiate a version, so
	-- answering it anyway is how two peers end up disagreeing about the wire while both believe
	-- they succeeded. Refusing is the only outcome that surfaces the mistake.
	t:expect(response.ok, "refused"):is_falsy()
	t:expect(response.outcome):equals("error")
end)

prova.test("an unknown operation is an error that names it", OPEN, function(t)
	local broker = placement.connect(t)
	broker:hello()

	local response = broker:request("teleport", {})

	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
	-- Naming the op is what makes a version mismatch diagnosable from one line of output rather
	-- than from a packet capture.
	t:expect(response.message, "the message names the op"):contains("teleport")
end)

prova.test("a malformed frame is rejected without killing the connection", OPEN, function(t)
	local broker = placement.connect(t)
	broker:hello()

	local response = broker:send_raw("{ this is not json")
	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")

	-- Surviving a bad frame matters because leases are held across turns: dropping the connection
	-- on a parse error would release every slot this client holds as a side effect of a typo.
	local after = broker:request("resolve", { capabilities = {} })
	t:expect(after.ok, "the connection still works"):is_true()
end)

prova.test("every terminal frame echoes the request id", OPEN, function(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	local resolved = broker:request("resolve", { capabilities = {} })

	-- Ids are what let a streaming op interleave with anything else on the connection. Without
	-- them, a client cannot tell whose response it just read.
	t:expect(hello.id, "hello echoed"):equals(1)
	t:expect(resolved.id, "resolve echoed"):equals(2)
end)

prova.test("the broker advertises its optional planes as a list", OPEN, function(t)
	local broker = placement.connect(t)
	local response = broker:hello()

	-- `features` is how a client knows not to send `exec` or `materialize` at a broker that cannot
	-- serve them. A broker with neither still answers with an empty list rather than omitting the
	-- key, so "no features" and "old broker that forgot to say" stay distinguishable.
	t:expect(response.ok):is_true()
	t:expect(type(response.features), "features is a list"):equals("table")
end)
