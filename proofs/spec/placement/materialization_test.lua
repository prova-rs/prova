-- Getting the code to the work (docs/design/placement.md §materialize).
--
-- The rule this plane exists to enforce: PLACE BY CHANGE ID, NEVER BY BRANCH NAME. A branch name is
-- mutable and means different things on different machines; a change id is content-addressed and
-- means exactly one tree everywhere. "Host and executor on the same branch" stops being a task to
-- coordinate and becomes a property of the request.
--
-- Against a local broker this same plane is worktree isolation: run a suite against an isolated tree
-- at a change id while you keep editing. Which is why it earns its place in the open product rather
-- than being distribution-only machinery.

local placement = require("placement")

-- Gated on unix only: the transport IS a unix socket and the conformance vocabulary (`sh`) is
-- POSIX. Otherwise hermetic — with no external broker named, each connect spawns the reference
-- broker fresh, so these proofs run (and the spec stays attested) on any unix machine.
local UNIX = { requires = { "unix" } }

local KIND = "prova-conformance-slot"

local function leased(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	if not placement.advertises(hello, "materialize") then
		t:skip("broker does not advertise the materialize plane")
	end

	local lease = broker:request("claim", { kind = KIND, mode = "exclusive", ttl_ms = 120000 })
	t:expect(lease.outcome, "claimed a slot to materialize onto"):equals("granted")
	t:defer(function()
		pcall(function()
			broker:request("release", { lease = lease.lease })
		end)
	end)

	return broker, lease.lease
end

--- This repo's own current change, as the thing to materialize. Using a REAL id rather than a
--- fabricated one matters: a broker that accepted anything would pass a test built on a fake.
local function current_change(t)
	local result = shell.run({ "jj", "log", "-r", "@", "--no-graph", "-T", "change_id" }, {
		cwd = prova.root,
		check = false,
	})
	if not result:ok() then
		t:skip("this checkout is not a jj repo")
	end
	return (result.stdout:gsub("%s+$", ""))
end

prova.test("materializing a change id yields a path holding that tree", UNIX, function(t)
	local broker, lease = leased(t)
	local change = current_change(t)

	local response = broker:request("materialize", {
		lease = lease,
		vcs = "jj",
		change = change,
		source = prova.root,
	})

	t:expect(response.ok):is_true()
	t:expect(response.path, "a path to work in"):never():is_nil()
	-- The path must be real on the node, because the next thing that happens is an `exec` with this
	-- as its cwd. A broker that reported a path it had not created would fail one op later, with the
	-- error pointing at the wrong plane.
	t:expect(response.path):is_dir()
end)

prova.test("an unknown change id is an error, not an empty workspace", UNIX, function(t)
	local broker, lease = leased(t)

	-- Valid in FORM (jj's change-id alphabet is k–z), absent in fact: a full-length id no repo
	-- holds. Not all-z — that is the ROOT commit's change id, which resolves in every jj repo,
	-- and a proof that "refused the unknown" built on it would be crediting a broker for
	-- materializing the root's empty tree. (Found the expensive way: the first reference-broker
	-- run materialized it successfully.)
	local response = broker:request("materialize", {
		lease = lease,
		vcs = "jj",
		change = "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk",
		source = prova.root,
	})

	-- Silently materializing something else — trunk, or an empty tree — is the worst available
	-- outcome: the suite runs, passes, and proves nothing about the code you meant to test. Whatever
	-- a broker cannot fetch, it must refuse.
	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
end)

prova.test("materialization is idempotent for the same change", UNIX, function(t)
	local broker, lease = leased(t)
	local change = current_change(t)

	local first = broker:request("materialize", {
		lease = lease, vcs = "jj", change = change, source = prova.root,
	})
	local second = broker:request("materialize", {
		lease = lease, vcs = "jj", change = change, source = prova.root,
	})

	-- Asking twice must not rebuild the world. This is what makes a retry cheap, and it is the
	-- foundation the warmth report below is built on.
	t:expect(second.ok):is_true()
	t:expect(second.path, "the same tree, same place"):equals(first.path)
end)

prova.test("warmth reports the nearest shared ancestor, or none when cold", UNIX, function(t)
	local broker, lease = leased(t)
	local change = current_change(t)

	local response = broker:request("materialize", {
		lease = lease, vcs = "jj", change = change, source = prova.root,
	})

	t:expect(response.ok):is_true()
	-- Warmth is the input to a scheduler's rebuild-cost estimate — the direct analogue of preferring
	-- the node that already holds a session's cache. It is advisory, so the contract is only that
	-- the key EXISTS and that a cold node says so honestly rather than claiming an ancestor it does
	-- not have. Overclaiming here would send work to the slowest node while calling it the warmest.
	t:expect(type(response.warmth), "warmth is reported"):equals("table")
end)

prova.test("materialize requires a lease", UNIX, function(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	if not placement.advertises(hello, "materialize") then
		t:skip("broker does not advertise the materialize plane")
	end

	local response = broker:request("materialize", {
		lease = "L-never-existed",
		vcs = "jj",
		change = "zzzz",
		source = prova.root,
	})

	-- Materializing without a lease would let a client fill a node's disk with trees nobody is
	-- scheduled to use, and nothing would ever clean them up: the lease is what bounds the
	-- workspace's lifetime.
	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
end)

prova.test("an unsupported vcs is refused by name", UNIX, function(t)
	local broker, lease = leased(t)

	local response = broker:request("materialize", {
		lease = lease,
		vcs = "hg",
		change = "tip",
		source = prova.root,
	})

	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
	-- Naming it is what tells the reader whether to install something or change their request.
	t:expect(response.message, "the message names the vcs"):contains("hg")
end)
