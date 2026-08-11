--- Black-box spec for the plugin registry (docs/design/registry.md): drive the `prova` binary
--- against a sandboxed XDG home whose config.toml lists local *path* registries — hermetic, no
--- network. A registry is a directory (normally a git repo) holding one TOML entry per plugin
--- under `registry/`; these specs pin the discovery surface, entry tolerance, add-time pinning,
--- and the hard line that discovery never changes what `require` resolves.

local sandbox = prova.fixture("registry-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()
  for _, dir in ipairs({
    "registries/main/registry", "registries/second/registry", "registries/override/registry",
    "config/prova", "config-empty/prova", "cache", "data", "projects",
  }) do
    fs.mkdir(root .. "/" .. dir)
  end

  -- The main registry: realistic entries plus the tolerance cases.
  fs.write(root .. "/registries/main/registry/postgres.toml", [[
schema       = 1
name         = "postgres"
repo         = "https://github.com/prova-rs/prova-postgres"
description  = "Postgres containers and direct SQL assertion via psql-in-image"
keywords     = ["postgres", "sql", "database", "container"]
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
keywords        = ["rabbitmq", "amqp", "queue"]
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

-- Run `prova plugins <args>` inside the sandbox. Pass `opts.merge` when the assertion is about
-- warnings/errors (folds stderr into stdout); leave it off when asserting what the row listing
-- does NOT contain.
local function plugins(sb, args, opts)
  opts = opts or {}
  return shell.run(prova.bin .. " packages " .. args, {
    cwd = opts.cwd or sb.root,
    env = sb.env(opts.config),
    merge_stderr = opts.merge,
  })
end

-- A fresh throwaway package for the add specs (add mutates prova.toml).
local function project(sb, name)
  local dir = sb.root .. "/projects/" .. name
  fs.mkdir(dir .. "/proofs")
  fs.write(dir .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  return dir
end

-- ── list & search ────────────────────────────────────────────────────────────────────────────

prova.test("`prova packages` lists entries from config-listed path registries", function(t)
  local sb = t:use(sandbox)
  -- cwd is the manifest-less sandbox root on purpose: discovery must work before a package
  -- exists, exactly like `prova init --list`.
  local r = plugins(sb, "")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("postgres")
  t:expect(r.stdout):contains("Postgres containers")  -- description rides the row
  t:expect(r.stdout):contains("rabbitmq")
end)

prova.test("with more than one registry configured, rows say which registry they came from", function(t)
  local sb = t:use(sandbox)
  local r = plugins(sb, "")
  t:expect(r.stdout):contains("main")
  t:expect(r.stdout):contains("second")
end)

prova.test("search matches on name", function(t)
  local sb = t:use(sandbox)
  local r = plugins(sb, "postgres")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("postgres")
  t:expect(r.stdout):never():contains("rabbitmq")
end)

prova.test("search matches on keywords, not just name", function(t)
  local sb = t:use(sandbox)
  -- "database" appears only in postgres's keywords — never in a name or description.
  local r = plugins(sb, "database")
  t:expect(r.stdout):contains("postgres")
  t:expect(r.stdout):never():contains("rabbitmq")
end)

prova.test("info shows the full entry: repo, recommended pin, requires, shape", function(t)
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
  { covers = "docs/design/registry.md#registry-entry-tolerance" }, function(t)
  local sb = t:use(sandbox)
  -- rabbitmq's entry carries `from_the_future`; it must list like any other entry.
  local r = plugins(sb, "rabbitmq")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("rabbitmq")
end)

prova.test("an entry with an unrecognized schema is skipped per-entry, with a warning",
  { covers = "docs/design/registry.md#registry-entry-tolerance" }, function(t)
  local sb = t:use(sandbox)
  local rows = plugins(sb, "")
  t:expect(rows.code):equals(0)                        -- the registry still serves
  t:expect(rows.stdout):contains("postgres")           -- siblings unaffected
  t:expect(rows.stdout):never():contains("futuristic") -- the schema-99 entry is not offered
  local warned = plugins(sb, "", { merge = true })
  t:expect(warned.stdout):contains("futuristic")       -- the skip names the entry
end)

prova.test("an entry missing a required field is skipped with a warning, not fatal",
  { covers = "docs/design/registry.md#registry-entry-tolerance" }, function(t)
  local sb = t:use(sandbox)
  local rows = plugins(sb, "")
  t:expect(rows.code):equals(0)
  t:expect(rows.stdout):never():contains("broken")
  local warned = plugins(sb, "", { merge = true })
  t:expect(warned.stdout):contains("broken")
end)

-- ── built-in default + offline ───────────────────────────────────────────────────────────────

prova.test("a user registry named after a built-in replaces it wholesale", function(t)
  local sb = t:use(sandbox)
  -- config.toml names `prova-rs` with a local path: listing must serve the override's entry
  -- (and succeed hermetically — nothing in the merged set can reach the network).
  local r = plugins(sb, "")
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("notreal")
end)

prova.test("the prova-rs registry is built in; offline with a cold cache it fails naming itself", function(t)
  local sb = t:use(sandbox)
  -- No user config at all: the built-in default is the whole set. Offline + never fetched →
  -- a clear error naming the registry it cannot serve, not a silent empty listing.
  local r = plugins(sb, "--offline", { config = "config-empty", merge = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("prova-rs")
end)

-- ── add: search-to-pinned in one motion ──────────────────────────────────────────────────────

prova.test("add writes a pinned [dependencies] entry using the recommended pin",
  { covers = "docs/design/registry.md#add-materializes-a-pin" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-latest")
  local r = plugins(sb, "add postgres", { cwd = proj })
  t:expect(r.code):equals(0)
  local manifest = fs.read(proj .. "/prova.toml")
  t:expect(manifest):contains("https://github.com/prova-rs/prova-postgres")
  t:expect(manifest):contains("v2")                    -- latest, materialized as the pin
end)

prova.test("add name@ref pins the explicit ref over latest",
  { covers = "docs/design/registry.md#add-materializes-a-pin" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-ref")
  local r = plugins(sb, "add postgres@v1", { cwd = proj })
  t:expect(r.code):equals(0)
  local manifest = fs.read(proj .. "/prova.toml")
  t:expect(manifest):contains("v1")
  t:expect(manifest):never():contains("v2")
end)

prova.test("a name in two registries demands registry:name disambiguation",
  { covers = "docs/design/registry.md#registry-name-disambiguation" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-ambiguous")
  local ambiguous = plugins(sb, "add dupe", { cwd = proj, merge = true })
  t:expect(ambiguous.code):never():equals(0)
  t:expect(ambiguous.stdout):contains("main")          -- the error names both candidates
  t:expect(ambiguous.stdout):contains("second")
  local qualified = plugins(sb, "add second:dupe", { cwd = proj })
  t:expect(qualified.code):equals(0)
  t:expect(fs.read(proj .. "/prova.toml")):contains("https://github.com/second-org/prova-dupe")
end)

prova.test("adding an unknown name is a clear error, not a guess", function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "add-unknown")
  local r = plugins(sb, "add nosuchplugin", { cwd = proj, merge = true })
  t:expect(r.code):never():equals(0)
  t:expect(r.stdout):contains("nosuchplugin")
  t:expect(fs.read(proj .. "/prova.toml")):never():contains("nosuchplugin")
end)

-- ── the discovery-only line ──────────────────────────────────────────────────────────────────

-- Already true today (no registry code exists to consult) and must STAY true after the registry
-- lands — this is the discovery-only guardrail, so it runs unflagged and holds the line
-- throughout the burndown.
prova.test("a registry-known name never resolves via require until the manifest declares it",
  { covers = "docs/design/registry.md#registry-is-discovery-only" }, function(t)
  local sb = t:use(sandbox)
  -- `dupe` exists in the configured registries but not in this package's [dependencies]; the
  -- searcher must not consult the registry (require's no-network safety boundary).
  local proj = project(sb, "discovery-only")
  -- prova.bin, not bare `prova`: this call replaces the environment via sb.env(), so PATH is not
  -- inherited. On a machine with prova installed the bare name resolved anyway and hid that; on a
  -- CI runner that only builds, it is `sh: prova: not found`. The absolute path is immune to both
  -- the replaced env and the changed cwd.
  local r = shell.run({ prova.bin, "eval", 'return (pcall(require, "dupe"))' }, {
    cwd = proj, env = sb.env(), merge_stderr = true,
  })
  t:expect(r.stdout):contains("false")
end)

-- ── the learn system announces the surface ───────────────────────────────────────────────────

prova.test("`prova learn plugins` teaches the registries and the search-first move",
  { covers = "docs/design/registry.md#learn-teaches-search-first" }, function(t)
  local sb = t:use(sandbox)
  local proj = project(sb, "learn-slot")
  local r = shell.run(prova.bin .. " learn plugins", { cwd = proj, env = sb.env(), merge_stderr = true })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("prova plugins")         -- the verb an agent should reach for
  t:expect(r.stdout):contains("main")                  -- the configured registries, rendered live
end)

-- ── the MCP mirror: one implementation, two transports ───────────────────────────────────────

prova.test("the MCP packages tool searches the same registries the CLI verb does",
  { covers = "docs/design/registry.md#registry-mcp-mirror" }, function(t)
  local sb = t:use(sandbox)

  -- The CLI leg: rows on stdout. "database" appears only in postgres's keywords, so the query
  -- exercises the full match (name + description + keywords), not a name prefix.
  local cli = plugins(sb, "database")
  t:expect(cli.code):equals(0)
  t:expect(cli.stdout):contains("postgres")
  t:expect(cli.stdout):never():contains("rabbitmq")

  -- The MCP leg: the same query through `prova mcp` stdio JSON-RPC, same sandboxed registries.
  local req = sb.root .. "/mirror-requests.jsonl"
  fs.write(req, table.concat({
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05",'
      .. '"capabilities":{},"clientInfo":{"name":"proof","version":"0"}}}',
    '{"jsonrpc":"2.0","method":"notifications/initialized"}',
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"packages",'
      .. '"arguments":{"query":"database"}}}',
  }, "\n") .. "\n")
  local r = shell.run(prova.bin .. " mcp < " .. req, {
    cwd = sb.root, env = sb.env(), timeout = "60s",
  })
  local answer
  for line in r.stdout:gmatch("[^\n]+") do
    local ok, msg = pcall(json.decode, line)
    if ok and type(msg) == "table" and msg.id == 2 then answer = msg end
  end
  t:expect(answer, "the packages tool answered"):is_truthy()
  local payload = json.decode(answer.result.content[1].text)

  -- Parity is the claim: the same one entry, with the fields the CLI's info view prints —
  -- one shared implementation serving both transports, not two searchers that happen to agree.
  t:expect(#payload.packages):equals(1)
  local entry = payload.packages[1]
  t:expect(entry.name):equals("postgres")
  t:expect(entry.registry):equals("main")
  t:expect(entry.repo):equals("https://github.com/prova-rs/prova-postgres")
  t:expect(entry.latest):equals("v2")
end)
