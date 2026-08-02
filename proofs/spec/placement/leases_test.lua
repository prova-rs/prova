-- Slot leases — the widening of `resources` (docs/design/placement.md §claim/renew/release).
--
-- `prova.writes("window-server")` is an exclusive hold. Locally that is an in-process semaphore, so
-- its parallelism is one. Against a broker it is a lease on a node-owned slot, so its parallelism
-- is however many nodes offer one. The declaration at the call site does not change — that is the
-- whole thesis, and these proofs are where it is held to.

local placement = require("placement")

local OPEN = {
	promises = "placement: no broker implementation yet (docs/design/placement.md)",
	requires = { "placement_broker" },
}

local KIND = "prova-conformance-slot"

local function connected(t)
	local broker = placement.connect(t)
	broker:hello()
	return broker
end

--- Claim and guarantee release, so one failing proof cannot strand a slot for the rest of the run.
local function claimed(t, broker, fields)
	local request = { kind = KIND, mode = "exclusive", ttl_ms = 60000 }
	for k, v in pairs(fields or {}) do
		request[k] = v
	end

	local response = broker:request("claim", request)
	if response.lease then
		t:defer(function()
			pcall(function()
				broker:request("release", { lease = response.lease })
			end)
		end)
	end
	return response
end

prova.test("a claim grants a lease with an identity and an expiry", OPEN, function(t)
	local broker = connected(t)
	local response = claimed(t, broker)

	t:expect(response.ok):is_true()
	t:expect(response.outcome):equals("granted")
	t:expect(response.lease, "the lease is addressable"):never():is_nil()
	-- An expiry is mandatory, not optional. It is the only thing standing between a killed prova
	-- and a slot held forever, and a GUI slot is a resource a human notices losing.
	t:expect(response.expires_at_ms, "the lease expires"):gt(0)
end)

prova.test("a second exclusive claim on a held slot is busy, never unsatisfiable", OPEN, function(t)
	local broker = connected(t)
	local first = claimed(t, broker)
	t:expect(first.outcome, "the first claim was granted"):equals("granted")

	local second = claimed(t, broker)

	-- THE rule. `busy` means wait; `unsatisfiable` means skip, and a skip is silent. Confusing them
	-- turns contention into a suite that reports green having tested nothing — which is strictly
	-- worse than a red build, because nobody investigates green.
	t:expect(second.outcome, "contention is not absence"):equals("busy")
	t:expect(second.retry_after_ms, "and it says when to come back"):gt(0)
end)

prova.test("shared mode admits concurrent holders where exclusive does not", OPEN, function(t)
	local broker = connected(t)

	-- prova.reads() vs prova.writes() already distinguishes these at the call site, so the slot
	-- grammar needs no new vocabulary — it needs the broker to honour the one that exists.
	local first = claimed(t, broker, { mode = "shared" })
	local second = claimed(t, broker, { mode = "shared" })

	t:expect(first.outcome):equals("granted")
	t:expect(second.outcome, "readers do not exclude each other"):equals("granted")
	t:expect(second.lease, "and they are distinct leases"):never():equals(first.lease)
end)

prova.test("an exclusive claim waits behind a shared one", OPEN, function(t)
	local broker = connected(t)
	local reader = claimed(t, broker, { mode = "shared" })
	t:expect(reader.outcome):equals("granted")

	local writer = claimed(t, broker, { mode = "exclusive" })

	-- Asymmetric on purpose: readers coexist, a writer excludes everyone. A broker that granted the
	-- writer anyway would let a UI test drive a window server another test is reading.
	t:expect(writer.outcome):equals("busy")
end)

prova.test("renewing extends the expiry", OPEN, function(t)
	local broker = connected(t)
	local lease = claimed(t, broker)
	t:expect(lease.outcome):equals("granted")

	local renewed = broker:request("renew", { lease = lease.lease })

	t:expect(renewed.ok):is_true()
	t:expect(renewed.expires_at_ms, "the deadline moved out"):gte(lease.expires_at_ms)
end)

prova.test("renewing an unknown lease is an error, never a fresh grant", OPEN, function(t)
	local broker = connected(t)
	local response = broker:request("renew", { lease = "L-never-existed" })

	-- The slot may already be held by someone else. Re-granting on a stale id is how a lease
	-- system double-books, and a double-booked GUI slot means two test runs driving one screen —
	-- which produces failures that reproduce for nobody.
	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
end)

prova.test("releasing frees the slot for the next claim", OPEN, function(t)
	local broker = connected(t)
	local first = broker:request("claim", { kind = KIND, mode = "exclusive", ttl_ms = 60000 })
	t:expect(first.outcome):equals("granted")

	local released = broker:request("release", { lease = first.lease })
	t:expect(released.ok, "released"):is_true()

	local second = claimed(t, broker)
	t:expect(second.outcome, "the slot came back"):equals("granted")
end)

prova.test("release is idempotent", OPEN, function(t)
	local broker = connected(t)
	local lease = broker:request("claim", { kind = KIND, mode = "exclusive", ttl_ms = 60000 })

	t:expect(broker:request("release", { lease = lease.lease }).ok):is_true()
	-- Teardown paths run twice more often than anyone intends — a deferred release plus an explicit
	-- one is the normal case, not an abuse. Erroring on the second would turn correct cleanup into
	-- a failing test.
	t:expect(broker:request("release", { lease = lease.lease }).ok, "twice is fine"):is_true()
end)

prova.test("an unknown slot kind is unsatisfiable, not busy", OPEN, function(t)
	local broker = connected(t)
	local response = broker:request("claim", {
		kind = "no-node-offers-this-xyzzy",
		mode = "exclusive",
		ttl_ms = 60000,
	})

	-- The mirror of the busy rule: a slot nobody offers will never come free, so telling the client
	-- to retry would hang the run forever. Absence skips; contention waits.
	t:expect(response.outcome):equals("unsatisfiable")
	t:expect(response.reason):never():is_nil()
end)

prova.test("a lease is never revoked out from under its holder", {
	promises = "placement: no broker implementation yet — drain semantics need a multi-node broker",
	requires = { "placement_broker" },
}, function(t)
	local broker = connected(t)
	local lease = claimed(t, broker)
	t:expect(lease.outcome):equals("granted")

	-- Drain, never preempt. A node leaving the pool stops GRANTING; it does not reclaim. A
	-- preempted test is indistinguishable from a failing test, so preemption would make a proof
	-- runner manufacture false reds. Request-scoped systems can retry transparently and so can
	-- afford preemption; a test run cannot.
	local renewed = broker:request("renew", { lease = lease.lease })
	t:expect(renewed.ok, "still ours"):is_true()
end)
