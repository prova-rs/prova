--- Black-box spec for the plugin registry (docs/design/registry.md): drive the `prova` binary
--- against a sandboxed XDG home whose config.toml lists local *path* registries — hermetic, no
--- network. A registry is a directory (normally a git repo) holding one TOML entry per plugin
--- under `registry/`; these specs pin the discovery surface, entry tolerance, add-time pinning,
--- and the hard line that discovery never changes what `require` resolves.

local sandbox = prova.fixture("registry-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  shell.run(
    "mkdir -p registries/main/registry registries/second/registry registries/override/registry "
      .. "config/prova config-empty/prova cache data projects",
    { cwd = root, check = true }
  )

  -- The main registry: realistic entries plus the tolerance cases.
  fs.write(root .. "/registries/main/registry/postgres.toml", [[
schema       = 1
name         = "postgres"
repo         = "https://github.com/prova-rs/prova-postgres"
description  = "Postgres containers and direct SQL assertion via psql-in-image"
capabilities = ["postgres", "sql", "database", "container"]
latest       = "v2"
namespaces   = ["postgres"]
shapes       = ["resource"]
requires     = ["docker"]
]])
  -- Carries a key no reader knows: graceful extensibility says it must be ignored, not fatal.
  fs.write(root .. "/registries/main/registry/rabbitmq.toml", [[
schema          = 1
name            = "rabbitmq"
repo            = "https://github.com/prova-rs/prova-rabbitmq"
description     = "RabbitMQ resource over rabbitmqadmin"
capabilities    = ["rabbitmq", "amqp", "queue"]
latest          = "v1"
from_the_future = { shiny = true }
]])
  -- A schema major this binary does not understand: skipped per-entry, with a warning.
  fs.write(root .. "/registries/main/registry/futuristic.toml", [[
schema      = 99
name        = "futuristic"
repo        = "https://example.com/futuristic"
description = "an entry from a newer registry generation"
]])
  -- Missing a required field (repo): skipped with a warning, never fatal to the registry.
  fs.write(root .. "/registries/main/registry/broken.toml", [[
schema      = 1
name        = "broken"
description = "an entry with no repo"
]])
  -- The same name in two registries — the ambiguity case for add.
  fs.write(root .. "/registries/main/registry/dupe.toml", [[
schema      = 1
name        = "dupe"
repo        = "https://github.com/main-org/prova-dupe"
description = "dupe as published by main"
latest      = "v1"
]])
  fs.write(root .. "/registries/second/registry/dupe.toml", [[
schema      = 1
name        = "dupe"
repo        = "https://github.com/second-org/prova-dupe"
description = "dupe as published by second"
latest      = "v3"
]])
  -- Replaces the built-in registry of the same name (see config.toml below).
  fs.write(root .. "/registries/override/registry/notreal.toml", [[
schema      = 1
name        = "notreal"
repo        = "https://github.com/prova-rs/prova-notreal"
description = "proof that the built-in was replaced by the user entry"
latest      = "v1"
]])

  -- User config: two path registries, plus an entry NAMED like the built-in — merge-by-name
  -- means it replaces the built-in wholesale, which is also what keeps every run here hermetic
  -- (nothing left in the set can reach the network).
  fs.write(root .. "/config/prova/config.toml", string.format([==[
[[registries]]
name   = "main"
source = "%s/registries/main"

[[registries]]
name   = "second"
source = "%s/registries/second"

[[registries]]
name   = "prova-rs"
source = "%s/registries/override"
]==], root, root, root))

  return {
    root = root,
    env = function(config_dir)
      return {
        XDG_CONFIG_HOME = root .. "/" .. (config_dir or "config"),
        XDG_CACHE_HOME  = root .. "/cache",
        XDG_DATA_HOME   = root .. "/data",
      }
    end,
  }
end)

-- Run `prova plugins <args>` inside the sandbox. Append `2>&1` in args when the assertion is
-- about warnings/errors; leave it off when asserting what the row listing does NOT contain.
local function plugins(sb, args, opts)
  opts = opts or {}
  return shell.run("prova plugins " .. args, {
    cwd = opts.cwd or sb.root,
    env = sb.env(opts.config),
  })
end

-- A fresh throwaway package for the add specs (add mutates prova.toml).
local function project(sb, name)
  local dir = sb.root .. "/projects/" .. name
  shell.run("mkdir -p " .. dir .. "/proofs", { check = true })
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  return dir
end

-- ── list & search ────────────────────────────────────────────────────────────────────────────

prova.test("`prova plugins` lists entries from config-listed path registries",
  { spec = "registry.md surface" }, function(t)
  local sb = t:use(sandbox)
  -- cwd is the manifest-less sandbox root on purpose: discovery must work before a package
  -- exists, exactly like `prova init --list`.
  local r = plugins(sb, "")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("postgres")
  t:expect(r.stdout):contains("Postgres containers")  -- description rides the row
  t:expect(r.stdout):contains("rabbitmq")
end)

prova.test("with more than one registry configured, rows say which registry they came from",
  { spec = "registry.md surface" }, function(t)
  local sb = t:use(sandbox)
  local r = plugins(sb, "")
  t:expect(r.stdout):contains("main")
  t:expect(r.stdout):contains("second")
end)

prova.test("search matches on name", { spec = "registry.md surface" }, function(t)
  local sb = t:use(sandbox)
  local r = plugins(sb, "postgres")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("postgres")
  t:expect(r.stdout):never():contains("rabbitmq")
end)

prova.test("search matches on capabilities, not just name",
  { spec = "registry.md surface" }, function(t)
  local sb = t:use(sandbox)
  -- "database" appears only in postgres's capabilities — never in a name or description.
  local r = plugins(sb, "database")
  t:expect(r.stdout):contains("postgres")
  t:expect(r.stdout):never():contains("rabbitmq")
end)

prova.test("info shows the full entry: repo, recommended pin, requires, shape",
  { spec = "registry.md surface" }, function(t)
  local sb = t:use(sandbox)
  local r = plugins(sb, "info postgres")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("https://github.com/prova-rs/prova-postgres")
  t:expect(r.stdout):contains("v2")
  t:expect(r.stdout):contains("docker")
  t:expect(r.stdout):contains("resource")
end)

-- ── entry tolerance (graceful extensibility) ─────────────────────────────────────────────────

prova.test("unknown keys in an entry are ignored, never fatal",
  { spec = "registry.md entry" }, function(t)
  local sb = t:use(sandbox)
  -- rabbitmq's entry carries `from_the_future`; it must list like any other entry.
  local r = plugins(sb, "rabbitmq")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("rabbitmq")
end)

prova.test("an entry with an unrecognized schema is skipped per-entry, with a warning",
  { spec = "registry.md entry" }, function(t)
  local sb = t:use(sandbox)
  local rows = plugins(sb, "")
  t:expect(rows.code):equals(0)                        -- the registry still serves
  t:expect(rows.stdout):contains("postgres")           -- siblings unaffected
  t:expect(rows.stdout):never():contains("futuristic") -- the schema-99 entry is not offered
  local warned = plugins(sb, "2>&1")
  t:expect(warned.stdout):contains("futuristic")       -- the skip names the entry
end)

prova.test("an entry missing a required field is skipped with a warning, not fatal",
  { spec = "registry.md entry" }, function(t)
  local sb = t:use(sandbox)
  local rows = plugins(sb, "")
  t:expect(rows.code):equals(0)
  t:expect(rows.stdout):never():contains("broken")
  local warned = plugins(sb, "2>&1")
  t:expect(warned.stdout):contains("broken")
end)

-- ── built-in default + offline ───────────────────────────────────────────────────────────────

prova.test("a user registry named after a built-in replaces it wholesale",
  { spec = "registry.md config" }, function(t)
  local sb = t:use(sandbox)
  -- config.toml names `prova-rs` with a local path: listing must serve the override's entry
  -- (and succeed hermetically — nothing in the merged set can reach the network).
  local r = plugins(sb, "")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("notreal")
end)

prova.test("the prova-rs registry is built in; offline with a cold cache it fails naming itself",
  { spec = "registry.md config" }, function(t)
  local sb = t:use(sandbox)
  -- No user config at all: the built-in default is the whole set. Offline + never fetched →
  -- a clear error naming the registry it cannot serve, not a silent empty listing.
  local r = plugins(sb, "--offline 2>&1", { config = "config-empty" })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("prova-rs")
end)

-- ── add: search-to-pinned in one motion ──────────────────────────────────────────────────────

prova.test("add writes a pinned [plugins] entry using the recommended pin",
  { spec = "registry.md add" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-latest")
  local r = plugins(sb, "add postgres", { cwd = proj })
  t:expect(r.code):equals(0)
  local manifest = fs.read(proj .. "/prova.toml")
  t:expect(manifest):contains("https://github.com/prova-rs/prova-postgres")
  t:expect(manifest):contains("v2")                    -- latest, materialized as the pin
end)

prova.test("add name@ref pins the explicit ref over latest",
  { spec = "registry.md add" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-ref")
  local r = plugins(sb, "add postgres@v1", { cwd = proj })
  t:expect(r.code):equals(0)
  local manifest = fs.read(proj .. "/prova.toml")
  t:expect(manifest):contains("v1")
  t:expect(manifest):never():contains("v2")
end)

prova.test("a name in two registries demands registry:name disambiguation",
  { spec = "registry.md add" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-ambiguous")
  local ambiguous = plugins(sb, "add dupe 2>&1", { cwd = proj })
  t:expect(ambiguous.code):never():equals(0)
  t:expect(ambiguous.stdout):contains("main")          -- the error names both candidates
  t:expect(ambiguous.stdout):contains("second")
  local qualified = plugins(sb, "add second:dupe", { cwd = proj })
  t:expect(qualified.code):equals(0)
  t:expect(fs.read(proj .. "/prova.toml")):contains("https://github.com/second-org/prova-dupe")
end)

prova.test("adding an unknown name is a clear error, not a guess",
  { spec = "registry.md add" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-unknown")
  local r = plugins(sb, "add nosuchplugin 2>&1", { cwd = proj })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("nosuchplugin")
  t:expect(fs.read(proj .. "/prova.toml")):never():contains("nosuchplugin")
end)

-- ── the discovery-only line ──────────────────────────────────────────────────────────────────

-- Already true today (no registry code exists to consult) and must STAY true after the registry
-- lands — this is the discovery-only guardrail, so it runs unflagged and holds the line
-- throughout the burndown.
prova.test("a registry-known name never resolves via require until the manifest declares it",
  function(t)
  local sb = t:use(sandbox)
  -- `dupe` exists in the configured registries but not in this package's [plugins]; the
  -- searcher must not consult the registry (require's no-network safety boundary).
  local proj = project(sb, "discovery-only")
  local r = shell.run([[prova eval 'return (pcall(require, "dupe"))' 2>&1]], {
    cwd = proj, env = sb.env(),
  })
  t:expect(r.stdout):contains("false")
end)

-- ── the learn system announces the surface ───────────────────────────────────────────────────

prova.test("`prova learn plugins` teaches the registries and the search-first move",
  { spec = "registry.md learn" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "learn-slot")
  local r = shell.run("prova learn plugins 2>&1", { cwd = proj, env = sb.env() })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("prova plugins")         -- the verb an agent should reach for
  t:expect(r.stdout):contains("main")                  -- the configured registries, rendered live
end)
