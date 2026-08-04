--- `prova run` — the lanes front door. A lane is a `[profiles.<name>]` table; the verb is sugar
--- for `--profile` (the primitive stays, and is what composes with other verbs), and
--- `prova run --list` answers "what can I run here" offline, from the manifest alone.

local sandbox = prova.fixture("lanes-sandbox", Scope.File, function(ctx)
	local root = ctx:tempdir()
	local proj = root .. "/pkg"
	fs.mkdir(proj .. "/proofs")
	fs.write(proj .. "/prova.toml", [[
[run]
proofs = ["proofs"]

[profiles.ut]
description = "the fast unit lane"

[profiles.bare]
]])
	fs.write(proj .. "/proofs/basic_test.lua", [[
prova.test("arithmetic holds", function(t)
  t:expect(1 + 1):equals(2)
end)
]])
	return proj
end)

prova.test("`prova run --list` shows the lanes, offline, with descriptions", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " run --list", { cwd = proj, merge_stderr = true })
	t:expect(r.code, r.stdout):equals(0)
	t:expect(r.stdout, "the default lane is named as what bare `prova` runs"):contains("(default)")
	t:expect(r.stdout):contains("ut")
	t:expect(r.stdout, "a declared description is the lane's line"):contains("the fast unit lane")
	t:expect(r.stdout, "an empty lane says so rather than showing nothing"):contains("no overrides")
	t:expect(r.stdout, "listing runs nothing"):never():contains("passed")
end)

prova.test("`prova run <lane>` is `--profile <lane>` — sugar, not a second code path", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " run ut", { cwd = proj, merge_stderr = true })
	t:expect(r.code, r.stdout):equals(0)
	t:expect(r.stdout):contains("1 passed")
	-- The primitive keeps working — the verb never replaces it.
	local p = shell.run(prova.bin .. " --profile ut", { cwd = proj, merge_stderr = true })
	t:expect(p.code, p.stdout):equals(0)
end)

prova.test("an unknown lane fails naming the ones that exist", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " run nope", { cwd = proj, merge_stderr = true })
	t:expect(r.code):never():equals(0)
	t:expect(r.stdout):contains("no such profile")
	t:expect(r.stdout, "the fix is on the error"):contains("ut")
end)

prova.test("a path handed to `run` gets the specific correction, not a profile error", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " run proofs/basic_test.lua", { cwd = proj, merge_stderr = true })
	t:expect(r.code):never():equals(0)
	t:expect(r.stdout, "names the right spelling"):contains("prova proofs/basic_test.lua")
end)

prova.test("bare `prova run` runs the default lane, exactly like bare `prova`", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " run", { cwd = proj, merge_stderr = true })
	t:expect(r.code, r.stdout):equals(0)
	t:expect(r.stdout):contains("1 passed")
end)
