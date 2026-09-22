-- Queued claims — the `queue` plane (docs/design/placement.md §Queued claims).
--
-- Polling a busy claim blocks the waiter for the whole wait and serves nobody in order. The queue
-- plane lets a client take a PLACE instead: FIFO, handed the slot by the broker, and told so by
-- refreshing a ticket whose every answer can be asked again. Every proof here needs two clients on ONE
-- broker, a holder and a waiter, so the address is resolved once and dialled twice.

local placement = require("placement")

--- UNIX plus a claim binding — every bound proof here covers one placement.md claim.
local function C(claim)
	return { requires = { "unix" }, covers = "docs/design/placement.md#" .. claim }
end

local KIND = "prova-conformance-slot"

--- A holder and a waiter on one broker, both greeted; skips when the broker does not serve the
--- plane (a client must not send an op the broker did not advertise).
local function pair(t)
	local addr = placement.broker(t)
	local holder = placement.connect(t, addr)
	local hello = holder:hello()
	if not placement.advertises(hello, "queue") then
		t:skip("broker does not advertise the queue plane")
	end
	local waiter = placement.connect(t, addr)
	waiter:hello()
	return holder, waiter
end

local function claim(client, fields)
	local request = { kind = KIND, mode = "exclusive", ttl_ms = 60000 }
	for k, v in pairs(fields or {}) do
		request[k] = v
	end
	return client:request("claim", request)
end

prova.test("the queue is an advertised plane", C("queue-is-a-plane"), function(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	-- A third-party broker may leave the plane out (it is optional); the reference broker, which
	-- keeps this spec attestable, must serve it.
	if placement.address() and not placement.advertises(hello, "queue") then
		t:skip("the broker under proof does not serve the queue plane")
	end
	t:expect(placement.advertises(hello, "queue"), "`queue` is advertised in features"):is_true()
	t:expect(hello.protocol, "and it is not a protocol bump"):equals(placement.PROTOCOL)
end)

prova.test("a busy slot queues in FIFO order, and a plain claim never jumps the queue", C("queued-is-fifo"), function(t)
	local holder, waiter = pair(t)
	local held = claim(holder)
	t:expect(held.outcome, "the holder was granted"):equals("granted")

	local first = claim(waiter, { queue = true })
	t:expect(first.outcome, "a queued claim on a busy slot takes a place"):equals("queued")
	t:expect(first.position, "the first place"):equals(1)
	t:expect(first.ticket, "a place is addressable"):never():is_nil()
	local second = claim(waiter, { queue = true })
	t:expect(second.position, "the second place, behind the first"):equals(2)

	holder:request("release", { lease = held.lease })
	local jumper = claim(holder)
	t:expect(jumper.outcome, "a plain claim while places wait is busy, even as the slot frees"):equals("busy")
end)

prova.test("a reader that could share the slot still waits behind a queued writer", C("queued-is-fifo"), function(t)
	-- The case FIFO exists for: readers coexist, so without the rule every new reader joins the
	-- held reader's slot and the queued writer never gets it.
	local holder, waiter = pair(t)
	local reader = claim(holder, { mode = "shared" })
	t:expect(reader.outcome, "a reader holds the slot"):equals("granted")
	local writer = claim(waiter, { queue = true })
	t:expect(writer.outcome, "a writer queues behind the reader"):equals("queued")
	local joiner = claim(holder, { mode = "shared" })
	t:expect(joiner.outcome, "a second reader waits behind the queued writer"):equals("busy")
end)

prova.test("the head of the queue is handed the slot, and the grant can be asked again", C("handed-not-hinted"), function(t)
	local holder, waiter = pair(t)
	local held = claim(holder)
	local place = claim(waiter, { queue = true })
	t:expect(waiter:request("ticket", { ticket = place.ticket }).outcome, "still waiting while held"):equals("queued")

	holder:request("release", { lease = held.lease })
	local granted = waiter:request("ticket", { ticket = place.ticket })
	t:expect(granted.outcome, "the release handed the slot to the head"):equals("granted")
	t:expect(granted.lease, "with its lease"):never():is_nil()
	local again = waiter:request("ticket", { ticket = place.ticket })
	t:expect(again.lease, "asked again, the same grant"):equals(granted.lease)
	t:expect(waiter:request("renew", { lease = granted.lease }).ok, "a handed lease renews like any other"):is_true()

	waiter:request("release", { lease = granted.lease })
	t:expect(waiter:request("ticket", { ticket = place.ticket }).ok, "an ended grant answers no ticket"):equals(false)
end)

prova.test("cancelling a handed ticket declines it to the next waiter", C("cancel-declines"), function(t)
	local holder, waiter = pair(t)
	local held = claim(holder)
	local a = claim(waiter, { queue = true })
	local b = claim(holder, { queue = true })
	holder:request("release", { lease = held.lease })
	t:expect(waiter:request("ticket", { ticket = a.ticket }).outcome, "a is handed the slot"):equals("granted")

	t:expect(waiter:request("cancel", { ticket = a.ticket }).ok, "a declines"):is_true()
	t:expect(waiter:request("cancel", { ticket = a.ticket }).ok, "and again: idempotent"):is_true()
	t:expect(holder:request("ticket", { ticket = b.ticket }).outcome, "b is handed it next"):equals("granted")
end)
