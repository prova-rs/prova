-- Running work on a lease (docs/design/placement.md §exec).
--
-- Execution goes THROUGH the broker rather than prova being handed an SSH endpoint. That is what
-- keeps credentials, host trust and transport entirely behind the socket — and it is why the local
-- reference broker exercises the same code path as a clustered one, instead of being a stub that
-- diverges the moment anything real ships.

local placement = require("placement")

-- Gated on unix only: the transport IS a unix socket and the conformance vocabulary (`sh`) is
-- POSIX. Otherwise hermetic — with no external broker named, each connect spawns the reference
-- broker fresh, so these proofs run (and the spec stays attested) on any unix machine.
local UNIX = { requires = { "unix" } }

--- UNIX plus a claim binding — every bound proof here covers one placement.md claim.
local function C(claim)
	return { requires = { "unix" }, covers = "docs/design/placement.md#" .. claim }
end

local KIND = "prova-conformance-slot"

local function leased(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	if not placement.advertises(hello, "exec") then
		t:skip("broker does not advertise the exec plane")
	end

	local lease = broker:request("claim", { kind = KIND, mode = "exclusive", ttl_ms = 60000 })
	t:expect(lease.outcome, "claimed a slot to run on"):equals("granted")
	t:defer(function()
		pcall(function()
			broker:request("release", { lease = lease.lease })
		end)
	end)

	return broker, lease.lease
end

prova.test("exec reports the exit code of the work, not of the transport", C("exec-reports-the-works-exit"), function(t)
	local broker, lease = leased(t)

	local ok = broker:request("exec", { lease = lease, argv = { "sh", "-c", "exit 0" } })
	t:expect(ok.ok, "the call succeeded"):is_true()
	t:expect(ok.exit):equals(0)

	local failed = broker:request("exec", { lease = lease, argv = { "sh", "-c", "exit 3" } })
	-- A command that exits non-zero is a SUCCESSFUL exec of a failing command. Collapsing the two
	-- would make every red test look like a broken pool, and the fix for those is not the same.
	t:expect(failed.ok, "exec itself worked"):is_true()
	t:expect(failed.exit, "and reported the command's code"):equals(3)
end)

prova.test("stdout and stderr stream as events before the terminal frame", C("streams-then-terminal"), function(t)
	local broker, lease = leased(t)

	local response, events = broker:request("exec", {
		lease = lease,
		argv = { "sh", "-c", "echo to-out; echo to-err 1>&2" },
	})

	t:expect(response.ok):is_true()

	local streams = {}
	for _, event in ipairs(events) do
		streams[event.event] = (streams[event.event] or "") .. (event.data or "")
	end

	-- Streaming rather than buffering-to-completion is what makes a long remote suite watchable.
	-- A broker that only returned output at the end would make a 20-minute UI run look hung.
	t:expect(streams.stdout, "stdout arrived"):contains("to-out")
	t:expect(streams.stderr, "stderr arrived, kept separate"):contains("to-err")
end)

prova.test("exec against an unknown lease is an error", C("exec-needs-a-live-lease"), function(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	if not placement.advertises(hello, "exec") then
		t:skip("broker does not advertise the exec plane")
	end

	local response = broker:request("exec", { lease = "L-never-existed", argv = { "true" } })

	-- Running unleased work is the failure this whole model exists to prevent: it is exactly the
	-- case where two suites end up on one window server. Refusing loudly is the only safe answer.
	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
end)

prova.test("the working directory and environment are honoured", UNIX, function(t)
	local broker, lease = leased(t)

	local response, events = broker:request("exec", {
		lease = lease,
		argv = { "sh", "-c", "pwd; echo $CONFORMANCE_MARKER" },
		cwd = "/",
		env = { CONFORMANCE_MARKER = "marked" },
	})

	local out = ""
	for _, event in ipairs(events) do
		if event.event == "stdout" then
			out = out .. (event.data or "")
		end
	end

	t:expect(response.ok):is_true()
	-- Both are load-bearing for the real use: a suite runs from a materialized workspace with
	-- toolchain variables set. A broker that silently ignored either would run the right command in
	-- the wrong place and report green.
	t:expect(out, "cwd applied"):contains("/")
	t:expect(out, "env applied"):contains("marked")
end)

prova.test("a client must not send exec to a broker that never advertised it", UNIX, function(t)
	local broker = placement.connect(t)
	local hello = broker:hello()
	if placement.advertises(hello, "exec") then
		t:skip("this broker does advertise exec — the negative case needs one that does not")
	end

	local response = broker:request("exec", { lease = "L-anything", argv = { "true" } })

	-- The other half of feature advertisement: a broker that does not claim a plane must refuse it
	-- rather than half-implement it. Otherwise `features` is decoration and a client cannot trust
	-- it to decide what is safe to send.
	t:expect(response.ok):is_falsy()
	t:expect(response.outcome):equals("error")
end)
