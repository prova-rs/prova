-- The CLIENT half of §Transport (docs/design/placement.md): where the broker address comes from,
-- and the one forbidden response to a broker that does not answer.
--
-- Everything here drives a child package through the real binary, because the claims are about
-- what a RUN does with configuration — not about any function. The child's proof writes a marker
-- file, so "the run never fell back to running locally" is observable as the marker's absence.

local placement = require("placement")

--- Gated on unix (the transport IS a unix socket) plus a claim binding — every proof here covers
--- one placement.md claim.
local function C(claim)
	return { requires = { "unix" }, covers = "docs/design/placement.md#" .. claim }
end

local scratch = prova.fixture("placement-transport-scratch", Scope.File, function(ctx)
	return function() return ctx:tempdir() end
end)

--- A child package whose single proof writes a marker when it RUNS. `broker` lands in
--- [placement]; the marker is how a forbidden local fallback would betray itself.
local function child(t, broker)
	local root = t:use(scratch)()
	local manifest = '[run]\nproofs = ["proofs"]\n'
	if broker then
		manifest = manifest .. '\n[placement]\nbroker = "' .. broker .. '"\n'
	end
	fs.write(root .. "/prova.toml", manifest)
	fs.write(root .. "/proofs/marker_test.lua", [[
prova.test("ran", function(t)
  fs.write(prova.root .. "/ran-marker", "the suite executed")
  t:expect(1):equals(1)
end)
]])
	return root
end

--- A dead-but-plausible address: nothing has ever listened there.
local function dead_socket()
	return "unix:///tmp/prova-dead-" .. uuid.v4():sub(1, 8) .. ".sock"
end

prova.test("a configured-but-unreachable broker fails the run loudly — local is never a fallback",
	C("unreachable-is-loud"), function(t)
	local dead = dead_socket()
	local root = child(t, dead)

	-- The env var is BLANKED, not unset: when this suite itself runs against an external broker
	-- (PROVA_PLACEMENT_BROKER in the outer environment), the child would inherit it and dial a
	-- live broker instead of the dead one under proof. Blank means unset, by the client's rule.
	local r = shell.run(prova.bin, {
		cwd = root, merge_stderr = true, env = { PROVA_PLACEMENT_BROKER = "" },
	})

	t:expect(r.code, "the run refused to proceed"):never():equals(0)
	t:expect(r.stdout):contains("cannot reach the placement broker")
	t:expect(r.stdout, "the error names the address"):contains(dead)
	t:expect(r.stdout, "and where it was configured"):contains("prova.toml")
	-- The forbidden outcome: a broken pool quietly becoming a local run. The marker proves no
	-- proof ever executed.
	t:expect(fs.exists(root .. "/ran-marker"), "nothing ran locally as a fallback"):equals(false)
end)

prova.test("a manifest-configured broker is dialed, announced, and the run proceeds",
	C("broker-address-resolution"), function(t)
	local live = placement.broker(t)
	local root = child(t, live)

	local r = shell.run(prova.bin, {
		cwd = root, merge_stderr = true, env = { PROVA_PLACEMENT_BROKER = "" },
	})

	t:expect(r.code):equals(0)
	t:expect(r.stdout, "the pool is announced"):contains("placement broker")
	t:expect(r.stdout, "with its size"):contains("1 node")
	t:expect(fs.exists(root .. "/ran-marker"), "and the suite ran"):equals(true)
end)

prova.test("the environment variable overrides the manifest",
	C("broker-address-resolution"), function(t)
	-- The manifest names a DEAD broker; the env var names a live one. If the manifest won, the
	-- run would die loudly — so a green run with the marker written is the precedence, observed.
	local live = placement.broker(t)
	local root = child(t, dead_socket())

	local r = shell.run(prova.bin, {
		cwd = root, merge_stderr = true, env = { PROVA_PLACEMENT_BROKER = live },
	})

	t:expect(r.code, "the env var's live broker won"):equals(0)
	t:expect(r.stdout):contains("placement broker")
	t:expect(fs.exists(root .. "/ran-marker")):equals(true)
end)
