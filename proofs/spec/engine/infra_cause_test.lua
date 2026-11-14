--- `prova.infra(msg)` marks a failed node's cause as the environment's, by value: the JSON event says
--- `cause = "infra"` and its line, so a gate tells an infra red from a real one without reading
--- prova's words (E m/11669, L m/11668). Only that call sets it: a proof's own error() and a failed
--- assertion are never infra, and the node is `failed` either way.

local sandbox = prova.fixture("infra-sandbox", Scope.File, function(ctx)
	local proj = ctx:tempdir() .. "/pkg"
	fs.mkdir(proj .. "/proofs")
	fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
	fs.write(proj .. "/proofs/causes_test.lua", [[
prova.test("the toolchain can't run", function(t) prova.infra("cargo metadata failed: error: offline\nmore") end)
prova.test("a proof's own error", function(t) error("a bug in the proof") end)
prova.test("a failed assertion", function(t) t:expect(1):equals(2) end)
]])
	return proj
end)

prova.test("only prova.infra marks a failure's cause as infra, with its first line", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " --format json", { cwd = proj })
	t:expect(r.code, "red, as ever"):never():equals(0)
	local by = {}
	for line in r.stdout:gmatch("[^\n]+") do
		local ok, e = pcall(json.decode, line)
		if ok and type(e) == "table" and e.type == "node_finished" then by[e.path] = e end
	end
	local infra = by["the toolchain can't run"] or {}
	t:expect(infra.outcome, "still failed"):equals("failed")
	t:expect(infra.cause, "infra, by value"):equals("infra")
	t:expect(infra.cause_line, "its first line"):equals("cargo metadata failed: error: offline")
	for _, name in ipairs({ "a proof's own error", "a failed assertion" }) do
		local e = by[name] or {}
		t:expect(e.outcome, name):equals("failed")
		t:expect(e.cause == nil or e.cause == json.null, name .. " is never infra"):equals(true)
	end
end)
