--- Black-box spec for archetype INDIRECTION: `prova init <key>` resolves a key to a repo through the
--- configured registries, so an archetype can live at any host under any repo name.
---
--- The thing being pinned is the absence of a convention. `prova init` used to reach its archetypes
--- through hardcoded URLs, and the guard protecting them asserted that each key matched
--- `prova-rs/prova-init-<key>-archetype`. That worked for prova's own two and was meaningless for
--- anyone else's — a third party could only participate by pasting a URL into their own config. A key
--- now RESOLVES rather than implying a URL, down a four-rung ladder:
---
---   1. `[init.<key>]` with a `source`   → used verbatim (no registry, no network)
---   2. `[init.<key>]` with no `source`  → looked up in the registries, local policy still applies
---   3. a built-in key                   → prova's own, explicitly pinned
---   4. a bare registry name             → never declared anywhere, zero config
---
--- Hermetic throughout: a sandboxed XDG home whose one registry is a LOCAL PATH replacing the built-in
--- `prova-rs` by name (merge-by-name drops the real one out of the set), and archetypes that are local
--- directories. Nothing here reaches the network.

local sandbox = prova.fixture("archetype-registry-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  for _, dir in ipairs({ "registry/archetypes", "config/prova", "renders" }) do
    fs.mkdir(root .. "/" .. dir)
  end

  -- A minimal but REAL archetype: renders a working prova package, so "resolved" is proven by the
  -- files that land rather than by a log line.
  local function archetype(dir, marker)
    fs.mkdir(dir .. "/contents/proofs")
    fs.write(dir .. "/archetype.yaml", '---\ndescription: "' .. marker .. '"\n'
      .. 'requires:\n  archetect: "3.0.0"\n')
    fs.write(dir .. "/archetype.lua",
      'local context = Context.new()\ndirectory.render("contents", context)\n')
    fs.write(dir .. "/contents/prova.toml", '[run]\nproofs = ["proofs"]\n')
    fs.write(dir .. "/contents/proofs/" .. marker .. "_test.lua",
      'prova.test("' .. marker .. '", function(t) t:expect(1):equals(1) end)\n')
  end

  archetype(root .. "/acme-arch", "acme")
  archetype(root .. "/rival-arch", "rival")

  -- The registry: an archetype nobody declares, plus one whose key collides with a built-in.
  fs.write(root .. "/registry/archetypes/acme-api.toml", string.format([[
schema = 1
name = "acme-api"
repo = "%s/acme-arch"
description = "An Acme API package"
in_package = "deny"
]], root))
  -- The same archetype, but the entry also recommends a pin. A registry may serve a local-path repo
  -- (the classification plugins use), and `path#ref` is not a path — so the pin must be dropped rather
  -- than concatenated, and said out loud.
  fs.write(root .. "/registry/archetypes/pinned-path.toml", string.format([[
schema = 1
name = "pinned-path"
repo = "%s/acme-arch"
description = "A local-path archetype that also recommends a pin"
latest = "v7"
]], root))
  fs.write(root .. "/registry/archetypes/project.toml", string.format([[
schema = 1
name = "project"
repo = "%s/rival-arch"
description = "A rival idea of what project means"
]], root))
  -- Tolerance: a newer schema must be skipped per-entry, never sink the registry.
  fs.write(root .. "/registry/archetypes/futuristic.toml", [[
schema = 99
name = "futuristic"
repo = "https://example.com/nope"
description = "from a newer registry generation"
]])

  --- Write config.toml with the local registry replacing the built-in, plus any `[init.*]` block.
  local function configure(init_blocks)
    fs.write(root .. "/config/prova/config.toml", string.format(
      '[[registries]]\nname = "prova-rs"\nsource = "%s/registry"\n\n%s',
      root, init_blocks or ""))
  end
  configure()

  return {
    root = root,
    configure = configure,
    env = function() return { XDG_CONFIG_HOME = root .. "/config" } end,
  }
end)

--- Run `prova init <args>` in a fresh destination directory; returns the result and that directory.
local function init(t, sb, args)
  local dest = sb.root .. "/renders/" .. tostring(t):gsub("%W", "") .. tostring(os.time())
    .. tostring(math.floor(os.clock() * 1e6))
  fs.mkdir(dest)
  local r = shell.run(prova.bin .. " init " .. args .. " --headless",
    { cwd = dest, env = sb.env(), merge_stderr = true })
  return r, dest
end

-- ── rung 4: the open namespace ───────────────────────────────────────────────────────────────

prova.test("a key nobody declared resolves through the registry and renders",
  { covers = "docs/design/registry.md#archetype-key-resolution" }, function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r, dest = init(t, sb, "acme-api")

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("registry prova-rs")     -- the origin is named, not silently assumed
  t:expect(r.stdout):contains("An Acme API package")   -- the publisher's description reaches the user
  t:expect(dest .. "/proofs/acme_test.lua"):exists()   -- and it really rendered
end)

prova.test("the resolved key never had to encode a repo name",
  { covers = "docs/design/registry.md#archetype-key-resolution" }, function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r = init(t, sb, "acme-api")
  -- The whole point: key `acme-api`, repo directory `acme-arch`. Nothing derives one from the other.
  t:expect(r.stdout):contains("acme-arch")
  t:expect(r.stdout):never():contains("prova-init-acme-api-archetype")
end)

-- ── rung 3 vs 4: a built-in is not shadowed by a registry entry ──────────────────────────────

prova.test("a registry entry cannot silently redefine a built-in key",
  { covers = "docs/design/registry.md#archetype-key-resolution" }, function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r = init(t, sb, "project")

  -- The registry serves `project` too, pointing at rival-arch. The built-in must win: publishing an
  -- archetype called `project` must not change what `prova init project` does everywhere it is listed.
  t:expect(r.stdout):contains("the catalog")
  t:expect(r.stdout):contains("prova-init-project-archetype")
  t:expect(r.stdout):never():contains("rival")
end)

-- ── rung 1: the explicit override, and rung 2: policy without a URL ──────────────────────────

prova.test("an explicit source overrides a built-in — the sanctioned way to redefine a key",
  function(t)
  local sb = t:use(sandbox)
  sb.configure(string.format(
    '[init.project]\ndescription = "Acme project"\nsource = "%s/rival-arch"\n', sb.root))
  local r, dest = init(t, sb, "project")

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("Acme project")
  t:expect(dest .. "/proofs/rival_test.lua"):exists()
end)

prova.test("a declared key with no source takes the registry's repo and keeps local policy",
  function(t)
  local sb = t:use(sandbox)
  -- Declared only to attach local wording — no URL pasted anywhere. This is the case that lets an org
  -- adopt someone else's published archetype without knowing where it lives.
  sb.configure('[init.acme-api]\ndescription = "Our API service"\n')
  local r, dest = init(t, sb, "acme-api")

  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("registry prova-rs")   -- sourced from the registry...
  t:expect(r.stdout):contains("Our API service")     -- ...described locally
  t:expect(dest .. "/proofs/acme_test.lua"):exists()
end)

-- ── the listing stays curated and offline ────────────────────────────────────────────────────

prova.test("--list shows the catalog and says the registries hold more", function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r = shell.run(prova.bin .. " init --list", { cwd = sb.root, env = sb.env(), merge_stderr = true })

  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("project")
  t:expect(r.stdout):contains("plugin")
  -- Undeclared registry archetypes are reachable but NOT listed: the list is the curated, offline,
  -- render-right-now set. Saying so is what stops it reading as the only options.
  t:expect(r.stdout):never():contains("acme-api")
  t:expect(r.stdout):contains("configured registry")
end)

-- ── pinning: `latest` is a recommendation the source has to be able to carry ─────────────────

prova.test("a local-path repo drops the recommended pin instead of corrupting the path",
  function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r, dest = init(t, sb, "pinned-path")

  -- `path#v7` would send archetect looking for a directory whose name ends in `#v7`. Appending a ref
  -- is only meaningful for a git source, so the render must use the bare path...
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):never():contains("#v7")
  t:expect(dest .. "/proofs/acme_test.lua"):exists()
  -- ...and must SAY the pin was dropped, because a silently unpinned render is not reproducible and
  -- the entry, not the user, is what asked for the pin.
  t:expect(r.stdout):contains("local path")
  t:expect(r.stdout):contains("unpinned")
end)

prova.test("an entry with no recommended pin says the render is unpinned", function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r = init(t, sb, "acme-api")
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("no `latest`")
end)

-- ── tolerance and diagnostics ────────────────────────────────────────────────────────────────

prova.test("an entry from a newer schema is skipped without sinking its siblings",
  { covers = "docs/design/registry.md#registry-entry-tolerance" }, function(t)
  local sb = t:use(sandbox)
  sb.configure()
  -- `futuristic` carries schema 99. Resolving a sibling must still work, which is the whole
  -- degrade-don't-break contract applied to the archetype half of a registry.
  local r = init(t, sb, "acme-api")
  t:expect(r.code, r.stdout):equals(0)
  t:expect(r.stdout):contains("futuristic")   -- reported...
  t:expect(r.stdout):contains("schema 99")    -- ...with why
end)

prova.test("an unknown key names the catalog AND says the registries were searched", function(t)
  local sb = t:use(sandbox)
  sb.configure()
  local r = init(t, sb, "no-such-archetype")

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("project")   -- what IS available
  -- "not found" must not read as "prova never looked" — the difference between a typo and a
  -- misconfigured registry list, which are fixed in completely different places.
  t:expect(r.stdout):contains("registry")
end)

prova.test("a declared key with no source and no registry hit explains both fixes", function(t)
  local sb = t:use(sandbox)
  sb.configure('[init.ghost]\ndescription = "nowhere to be found"\n')
  local r = init(t, sb, "ghost")

  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("source")
  t:expect(r.stdout):contains("config.toml")
end)
