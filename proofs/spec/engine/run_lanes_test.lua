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
tags = ["unit", "!slow"]

[profiles.bare]
]])
	fs.write(proj .. "/proofs/basic_test.lua", [[
prova.test("arithmetic holds", function(t)
  t:expect(1 + 1):equals(2)
end)

prova.test("fast unit check", { tags = { "unit" } }, function(t)
  t:expect(true):is_true()
end)

prova.test("slow unit soak", { tags = { "unit", "slow" } }, function(t)
  t:expect(true):is_true()
end)

prova.test("the tree lints clean", { tags = { "lint" } }, function(t)
  t:expect(true):is_true()
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
	-- The lane's baked tags select: unit ∧ ¬slow → exactly one of the four tests.
	t:expect(r.stdout):contains("1 passed")
	t:expect(r.stdout):contains("fast unit check")
	t:expect(r.stdout):never():contains("slow unit soak")
	t:expect(r.stdout):never():contains("lints clean")
	-- The primitive keeps working — the verb never replaces it.
	local p = shell.run(prova.bin .. " --profile ut", { cwd = proj, merge_stderr = true })
	t:expect(p.code, p.stdout):equals(0)
end)

prova.test("CLI selection narrows WITHIN the lane, and can never escape it", function(t)
	local proj = t:use(sandbox)
	-- Narrowing inside the lane: a keyword that matches the lane's one test.
	local narrowed = shell.run(prova.bin .. ' run ut -k "fast"', { cwd = proj, merge_stderr = true })
	t:expect(narrowed.code, narrowed.stdout):equals(0)
	t:expect(narrowed.stdout):contains("1 passed")
	-- Escaping: selecting the lint test from inside the ut lane must select NOTHING — the lane
	-- gate is ANDed, so the CLI narrows the set but never widens past it.
	local escape = shell.run(prova.bin .. ' run ut --tags lint --allow-empty',
		{ cwd = proj, merge_stderr = true })
	t:expect(escape.code, escape.stdout):equals(0)
	t:expect(escape.stdout):contains("0 passed")
	t:expect(escape.stdout):never():contains("lints clean")
end)

prova.test("the listing stays a listing whatever the lanes carry", function(t)
	local proj = t:use(sandbox)
	local r = shell.run(prova.bin .. " run --list", { cwd = proj, merge_stderr = true })
	-- A declared description wins the lane's line (tags chip only when there is none).
	t:expect(r.stdout):contains("the fast unit lane")
	t:expect(r.stdout, "listing runs nothing"):never():contains("passed")
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
	t:expect(r.stdout):contains("4 passed")
end)
