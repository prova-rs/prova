--- `ctx:tempdir(name?)` addresses a directory; it never manufactures one
--- (docs/design/agent-ergonomics.md#context-tempdir-not-idempotent).
---
--- The verb reads as an accessor — it addresses the scope, the way `ctx:use`, `ctx:defer` and
--- `ctx:log` do — and it used to be a FACTORY, handing back a fresh empty directory per call. So
--- `fs.write(t:tempdir() .. "/cookies.txt", …)` followed by `fs.read(t:tempdir() ..
--- "/cookies.txt")` wrote one directory and read another. Nothing errored: reading a missing path
--- in a fresh directory simply yields nothing, and the proof failed much later on whatever
--- consumed the result. In the case that surfaced it that was a curl cookie jar, so a login flow
--- appeared to be rejected by the identity provider when the session cookie had merely been
--- written somewhere the next step never looked. About an hour, chasing an auth bug that did not
--- exist.
---
--- The name carries the "I need several" case the old factory was really being used for — without
--- giving up idempotence, because a name is answered consistently forever. That matters more than
--- it sounds: fifteen call sites in this repo had hand-rolled a counter and a subdirectory to get
--- distinct roots, which is the shape of a missing primitive.
---
--- Every case runs in a SANDBOX PACKAGE driven by `prova.bin`. `ctx:tempdir` is runtime behavior,
--- and runtime behavior asserted in a proof body would exercise whichever prova is CONDUCTING this
--- suite — usually an installed one — which is how a proof goes green on the very commit that
--- breaks the feature (CLAUDE.md, "the subtler form of the same trap").

local scaffold = require("scaffold")

--- Run one sandbox proof through the subject and hand back its result.
local function ran(t, proof)
  local proj = scaffold.package(t, { name = "sandbox", proofs = { ["sandbox_test.lua"] = proof } })
  return shell.run(prova.bin, { cwd = proj, merge_stderr = true, timeout = "60s" })
end

prova.test("asking twice, by the same name or by none, is asking for the same directory", {
  covers = "docs/design/agent-ergonomics.md#context-tempdir-not-idempotent",
  proves = "idempotence is the whole contract, and it has to hold at BOTH arities — a named directory that drifted would be the original defect wearing a parameter",
}, function(t)
  local r = ran(t, [[
prova.test("two asks, one directory", function(t)
  t:expect(t:tempdir(), "the unnamed one is stable"):equals(t:tempdir())
  t:expect(t:tempdir("plugin"), "…and so is a named one"):equals(t:tempdir("plugin"))
end)
]])
  t:expect(r.code, "the subject holds the contract: " .. r.stdout):equals(0)
end)

prova.test("what one call writes, another call reads", {
  covers = "docs/design/agent-ergonomics.md#context-tempdir-not-idempotent",
  proves = "the failure was never 'two paths differ' — it was a read that silently found nothing, so the assertion has to go through the FILESYSTEM the way the field case did, not merely compare strings",
}, function(t)
  local r = ran(t, [[
prova.test("a cookie jar written in setup is there at the read", function(t)
  fs.write(t:tempdir() .. "/cookies.txt", "session=abc123")
  -- Deliberately a fresh call, exactly as an author writing setup and assertion apart would.
  t:expect(fs.read(t:tempdir() .. "/cookies.txt"), "the second call reads what the first wrote")
    :equals("session=abc123")
end)
]])
  t:expect(r.code, "the subject holds the contract: " .. r.stdout):equals(0)
end)

prova.test("different names are different directories, and they do not leak into each other", {
  covers = "docs/design/agent-ergonomics.md#context-tempdir-not-idempotent",
  proves = "this is the case the old factory served and a bare memo would have removed: a proof that stands up a plugin AND its consumer needs two roots, and needs each to contain only what it was given — distinct PATHS alone would not be enough if the trees overlapped",
}, function(t)
  local r = ran(t, [[
prova.test("a plugin and its consumer", function(t)
  local plugin, consumer = t:tempdir("plugin"), t:tempdir("consumer")
  t:expect(plugin, "two names, two directories"):never():equals(consumer)
  t:expect(plugin, "…and neither is the unnamed one"):never():equals(t:tempdir())

  fs.write(plugin .. "/prova.toml", "# the plugin\n")
  t:expect(fs.exists(consumer .. "/prova.toml"), "the consumer sees none of the plugin's files")
    :is_false()
end)
]])
  t:expect(r.code, "the subject holds the contract: " .. r.stdout):equals(0)
end)

prova.test("the name reaches the directory's own path, so a failed run is readable on disk", {
  covers = "docs/design/agent-ergonomics.md#context-tempdir-not-idempotent",
  proves = "the hour this cost was spent asking WHICH directory the run had actually written to; three sandboxes under indistinguishable hex names leave that question to be re-derived from the proof, and a name in the path answers it with `ls`",
}, function(t)
  local r = ran(t, [[
prova.test("named on disk", function(t)
  print("PLUGIN=" .. t:tempdir("plugin"))
  -- A name that is not path-safe must not escape the temp root — it is sanitized, not honored.
  print("HOSTILE=" .. t:tempdir("../escape hatch"))
end)
]])
  t:expect(r.code, "the subject ran: " .. r.stdout):equals(0)
  local plugin = r.stdout:match("PLUGIN=(%S+)")
  t:expect(plugin, "the name is in the path: " .. tostring(plugin)):contains("plugin")

  local hostile = r.stdout:match("HOSTILE=(%S+)")
  t:expect(hostile, "a traversal is not honored"):never():contains("..")
  t:expect(hostile, "…nor a separator that would nest it elsewhere"):never():contains("escape/")
  t:expect(hostile, "…and it still lands under a prova temp root"):contains("prova-")
end)

prova.test("the sandbox builder asks for a distinct directory every time", {
  covers = "docs/design/agent-ergonomics.md#context-tempdir-not-idempotent",
  proves = "`scaffold.package` is where fifteen call sites' worth of this need was consolidated, so the key it passes is worth pinning: sharing a root does not fail loudly, it overwrites, and the proof then asserts against a package it never built",
}, function(t)
  -- A STUB context, deliberately. What belongs to scaffold is which KEY it asks for — the
  -- directory that key resolves to belongs to the runtime, and is proven above through
  -- `prova.bin`. Asserting the resolved path here instead would read the CONDUCTING binary's
  -- `tempdir` and go red on an older one for reasons that say nothing about this helper.
  local asked = {}
  local probe = t:tempdir("scaffold-probe")
  local fake = {
    tempdir = function(_, key)
      asked[#asked + 1] = key
      local dir = probe .. "/" .. #asked
      fs.mkdir(dir)
      return dir
    end,
  }

  scaffold.package(fake, { name = "alpha", proofs = { ["a_test.lua"] = 'prova.test("a", function(t) end)\n' } })
  t:expect(asked[1], "a caller's name is what the directory is asked for by"):equals("alpha")

  -- Unnamed callers must still get distinct roots — the old one-sandbox call shape is everywhere,
  -- and it must not silently collide now that the key decides identity.
  scaffold.package(fake, {})
  scaffold.package(fake, {})
  t:expect(asked[2], "two unnamed packages ask for two directories"):never():equals(asked[3])
end)

prova.test("the memo is per scope instance, so a fixture and its test do not share", {
  covers = "docs/design/agent-ergonomics.md#context-tempdir-not-idempotent",
  proves = "a per-RUN memo would read identically in the happy case while quietly letting one test see another's scratch files — this is the assertion that separates the fix from a worse bug wearing its clothes",
}, function(t)
  local r = ran(t, [[
local workspace = prova.fixture("workspace", Scope.File, function(ctx)
  fs.write(ctx:tempdir() .. "/marker", "from the fixture")
  return { dir = ctx:tempdir() }
end)

prova.test("a fixture's tempdir is its own scope's", function(t)
  local ws = t:use(workspace)
  t:expect(fs.read(ws.dir .. "/marker"), "the fixture saw one directory throughout")
    :equals("from the fixture")
  t:expect(t:tempdir(), "the test's own scratch is a different scope's"):never():equals(ws.dir)
end)
]])
  t:expect(r.code, "the subject holds the contract: " .. r.stdout):equals(0)
end)
