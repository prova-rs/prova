--- The globals model (docs/plans/platform-agnostic-apis.md) — ONE canonical namespace `prova.*`,
--- with unqualified ambient globals DECLARED per package. Four mechanisms close every silent-collision
--- path between bundled modules, plugins, and user code:
---
---   1. canonical `prova.*` — every bundled module (and every declared plugin) is `prova.<name>`,
---      always resolvable whether or not it is injected. `prova` is the only guaranteed global.
---   2. declared injection — `[globals] inject = [...]` binds the listed names as unqualified ambient
---      globals (the DSL sugar). Bundled and plugin names inject by the SAME one line. Only injected
---      names are write-protected; every other name is the user's.
---   3. reserved-name registry — a `[plugins]` entry or plugin-root file bearing a bundled module
---      name is a MANIFEST VALIDATION error, never a silent shadow.
---   4. injection is sugar over `require` — an un-injected module is still `require`-able by name and
---      still `prova.<name>`; injection never adds or removes a capability.
---
--- Probes that could clobber the parent's own globals run in a sandbox child package (the spec-engine
--- idiom), not in-process. `fs.write` auto-creates parents, so these need no `mkdir` shell-out.

local sandbox = prova.fixture("globals-sandbox", Scope.File, function(ctx)
  return ctx:tempdir()
end)

--- Write a child package. `files` maps repo-relative path → contents; every parent dir is created by
--- `fs.write`, so no `mkdir` is needed (and none of this shells out — it runs on every platform).
local function child(t, name, manifest, files)
  local proj = t:use(sandbox) .. "/" .. name
  fs.write(proj .. "/prova.toml", manifest)
  for rel, body in pairs(files or {}) do
    fs.write(proj .. "/" .. rel, body)
  end
  return proj
end

-- ── 1. canonical prova.* — always available, the same object as require ───────────────────────

prova.test("every bundled module is prova.<name>, and it is THE namespace",
  { proves = "api-injection-model: prova.* is the canonical surface" }, function(t)
  t:expect(type(prova.fs.write)):equals("function")
  t:expect(prova.fs == require("fs")):is_true()          -- the same object, not a copy
end)

-- ── 2. declared injection — [globals] inject binds unqualified globals ─────────────────────────

prova.test("an injected name is an unqualified global; prova.<name> still works too",
  { proves = "api-injection-model: [globals] inject binds an unqualified ambient global" }, function(t)
  local proj = child(t, "inject-fs", '[run]\nproofs = ["proofs"]\n\n[globals]\ninject = ["fs"]\n',
    { ["proofs/probe_test.lua"] = [[
prova.test("fs is unqualified, and it is prova.fs", function(t)
  t:expect(type(fs.write)):equals("function")
  t:expect(fs == prova.fs):is_true()
end)
]] })
  t:expect(shell.run(prova.bin, { cwd = proj }):ok()):is_true()
end)

prova.test("a name NOT in inject is not an unqualified global, but prova.<name> and require still reach it",
  { proves = "api-injection-model: un-injected is not global, but stays canonical + require-able" }, function(t)
  local proj = child(t, "no-inject-fs", '[run]\nproofs = ["proofs"]\n\n[globals]\ninject = ["shell"]\n',
    { ["proofs/probe_test.lua"] = [[
prova.test("fs is not an ambient global here, but is reachable canonically", function(t)
  t:expect(fs == nil):is_true()                          -- not injected → not an unqualified global
  t:expect(type(prova.fs.write)):equals("function")      -- canonical form always works
  t:expect(type(require("fs").write)):equals("function") -- require-able under any local name
end)
]] })
  t:expect(shell.run(prova.bin, { cwd = proj }):ok()):is_true()
end)

-- ── 3. write-protection tracks the INJECTED set ───────────────────────────────────────────────

prova.test("assigning an injected name raises with guidance; a non-injected name is the user's",
  { proves = "api-injection-model: only injected names are write-protected" }, function(t)
  local proj = child(t, "write-protect", '[run]\nproofs = ["proofs"]\n\n[globals]\ninject = ["fs"]\n',
    { ["proofs/probe_test.lua"] = [[
prova.test("assignment to the injected 'fs' raises, teaching the out", function(t)
  local ok, err = pcall(function() fs = {} end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):contains("prova namespace")
  t:expect(tostring(err)):contains("inject")             -- points at [globals] inject
end)

prova.test("a non-injected name ('http' here) is an ordinary global the user owns", function(t)
  http = { marker = true }                                -- not injected → assignment is fine
  t:expect(http.marker):is_true()
end)

prova.test("local shadowing of an injected name stays lexical and legal", function(t)
  local fs = { marker = true }
  t:expect(fs.marker):is_true()
end)
]] })
  t:expect(shell.run(prova.bin, { cwd = proj }):ok()):is_true()
end)

-- ── 4. uniform participation — a plugin injects exactly like a bundled module ──────────────────

prova.test("a declared plugin injects by the same [globals] inject line — but does NOT join prova.*",
  { proves = "api-injection-model: a plugin injects like a bundled module; prova.* stays first-party" }, function(t)
  local proj = child(t, "inject-plugin",
    '[run]\nproofs = ["proofs"]\n\n[plugins]\ngreet = "./greet"\n\n[globals]\ninject = ["fs", "greet"]\n',
    {
      ["greet/init.lua"] = 'return { hello = function() return "hi" end }\n',
      ["proofs/probe_test.lua"] = [[
prova.test("the plugin is unqualified (injected) and require-able — but not under prova.*", function(t)
  t:expect(greet.hello()):equals("hi")                   -- injected → unqualified, same line as fs
  t:expect(require("greet").hello()):equals("hi")        -- and require-able, as always
  t:expect(prova.greet == nil):is_true()                 -- prova.* is first-party only; no prova.greet
end)
]],
    })
  t:expect(shell.run(prova.bin, { cwd = proj }):ok()):is_true()
end)

-- ── 5. reserved-name registry — a plugin may not claim a bundled module name ───────────────────

prova.test("a [plugins] entry bearing a bundled module name is a manifest validation error",
  { proves = "api-injection-model: a bundled name is a validation error, never a silent shadow" }, function(t)
  local proj = child(t, "reserved-plugin",
    '[run]\nproofs = ["proofs"]\n\n[plugins]\nfs = "./fsplug"\n',
    { ["fsplug/init.lua"] = "return {}\n" })
  local r = shell.run(prova.bin, { cwd = proj })
  t:expect(r.code):never():equals(0)
  t:expect(r.stderr .. r.stdout):contains("fs")
  t:expect(r.stderr .. r.stdout):contains("reserved")
end)

-- ── 6. inject validation & defaults ───────────────────────────────────────────────────────────

prova.test("injecting an unknown name (not a bundled module or declared plugin) is a manifest error",
  { proves = "api-injection-model: the inject list is validated against known modules + plugins" }, function(t)
  local proj = child(t, "inject-unknown",
    '[run]\nproofs = ["proofs"]\n\n[globals]\ninject = ["nope"]\n', {})
  local r = shell.run(prova.bin, { cwd = proj })
  t:expect(r.code):never():equals(0)
  t:expect(r.stderr .. r.stdout):contains("nope")
end)

prova.test("with no [globals] section, the sensible defaults are injected",
  { proves = "api-injection-model: absent [globals] → default inject set" }, function(t)
  local proj = child(t, "defaults", '[run]\nproofs = ["proofs"]\n',
    { ["proofs/probe_test.lua"] = [[
prova.test("fs is unqualified by default", function(t)
  t:expect(type(fs.write)):equals("function")
end)
]] })
  t:expect(shell.run(prova.bin, { cwd = proj }):ok()):is_true()
end)
