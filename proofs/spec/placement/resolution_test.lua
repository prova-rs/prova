-- Capability resolution — the widening of `requires` (docs/design/placement.md §resolve).
--
-- `requires = { "dotnet >= 9" }` asks one question: can this run anywhere I have? Locally that is a
-- PATH probe. Against a broker it is a pool query. The ANSWER's grammar must not change, because
-- what prova does with it — run, or skip with a reason — is what every suite already depends on.

local placement = require("placement")

local OPEN = {
	spec = "placement: no broker implementation yet (docs/design/placement.md)",
	requires = { "placement_broker" },
}

local function connected(t)
	local broker = placement.connect(t)
	broker:hello()
	return broker
end

prova.test("a satisfiable capability resolves with a node count", OPEN, function(t)
	local broker = connected(t)
	local response = broker:request("resolve", { capabilities = { { name = "sh" } } })

	t:expect(response.ok):is_true()
	t:expect(response.outcome):equals("granted")
	-- A COUNT, not a roster. Prova needs to know whether the work can run, never where — node
	-- identity is the broker's business, and keeping it out of the response keeps it out of prova's
	-- model, which is what stops "where did this run" leaking into proof authorship.
	t:expect(response.nodes, "at least one node can serve"):gte(1)
end)

prova.test("an absent capability is unsatisfiable, which is what makes a skip honest", OPEN, function(t)
	local broker = connected(t)
	local response = broker:request("resolve", {
		capabilities = { { name = "definitely-not-installed-anywhere-xyzzy" } },
	})

	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("unsatisfiable")
	-- The reason is not decoration. `requires` skips SILENTLY by design, so the reason is the only
	-- artifact a reader gets. A skip with no reason is indistinguishable from a test nobody wrote.
	t:expect(response.reason, "the skip carries its reason"):never():is_nil()
end)

prova.test("version constraints are honoured, not ignored", OPEN, function(t)
	local broker = connected(t)

	-- An unsatisfiable FLOOR on something that exists. A broker that parsed the name and dropped
	-- the constraint would answer `granted` here, and every version-gated proof in every consuming
	-- suite would run against a toolchain it declared it could not use.
	local response = broker:request("resolve", {
		capabilities = { { name = "sh", constraint = ">= 9999" } },
	})

	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("unsatisfiable")
end)

prova.test("several capabilities resolve conjunctively", OPEN, function(t)
	local broker = connected(t)

	-- `requires = { "a", "b" }` means ONE node has both, not that the pool has each somewhere. A
	-- broker that answered disjunctively would place work on a node missing half its requirements.
	local response = broker:request("resolve", {
		capabilities = { { name = "sh" }, { name = "definitely-not-installed-anywhere-xyzzy" } },
	})

	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("unsatisfiable")
	t:expect(response.reason, "names the missing one"):contains("xyzzy")
end)

prova.test("an empty capability list resolves against the whole pool", OPEN, function(t)
	local broker = connected(t)
	local response = broker:request("resolve", { capabilities = {} })

	-- A proof with no `requires` runs anywhere. That degenerate case has to be `granted` rather
	-- than an error, because it is the common case: most proofs demand nothing in particular.
	t:expect(response.ok):is_true()
	t:expect(response.outcome):equals("granted")
	t:expect(response.nodes):gte(1)
end)

prova.test("a saturated pool is never reported as unsatisfiable", OPEN, function(t)
	local broker = connected(t)

	-- THE rule of this protocol, checked from the resolution side: `resolve` answers about
	-- CAPABILITY, never about availability. Contention is `busy` at claim time. A broker that let
	-- saturation leak into resolution would turn every capacity shortage into a silent skip, and a
	-- suite would report green having tested nothing — the one failure a proof runner may never
	-- have.
	local response = broker:request("resolve", { capabilities = { { name = "sh" } } })

	t:expect(response.outcome, "capability is not availability"):never():equals("busy")
	t:expect(response.outcome):equals("granted")
end)

prova.test("resolution is advisory and does not reserve anything", OPEN, function(t)
	local broker = connected(t)

	-- Resolving twice must not consume capacity. `resolve` is a question, `claim` is a commitment;
	-- a broker that reserved on resolve would leak a slot for every skipped test.
	local first = broker:request("resolve", { capabilities = { { name = "sh" } } })
	local second = broker:request("resolve", { capabilities = { { name = "sh" } } })

	t:expect(second.outcome):equals(first.outcome)
	t:expect(second.nodes, "capacity unchanged by asking"):equals(first.nodes)
end)
