---@meta prova.cargo
-- `require("prova.cargo")` — the Cargo module adapter (docs/plans/module-proofs.md). A workspace
-- member with its own `proofs/` is a MODULE, keyed by the digest of its build closure; a module
-- whose key is unchanged since a fully green conduct is served from the conduct cache, and only
-- stale modules are built and tested, in one nextest run. Proofs always execute: only the conduct
-- is cached. Declared in prova.toml as `[modules] from = ["cargo"]`.

---@class prova.CargoConfigureOpts
---@field profile? string     # the nextest profile; its junit must be enabled (default "default")
---@field junit? string       # that profile's junit file name, under <target>/nextest/<profile>/
---@field args? string[]      # extra `cargo nextest run` arguments (features, --test-threads, …)
---@field run? table          # extra `shell.run` options for the conduct: `env`, `sample`, `idle_timeout`

---@class prova.cargo
local cargo = {}

--- Set the conduct's nextest profile, junit file name and extra arguments, once, where the
--- package's proofs share setup: every state that may conduct must agree, since whichever asks
--- first conducts for the run.
---@param opts prova.CargoConfigureOpts
function cargo.configure(opts) end

--- The account of module `name` (default: this proof's module) — a junit path, for `junit.load`
--- readers that bind claims to named cases.
---@param t prova.TestContext
---@param name? string
---@return string
function cargo.account(t, name) end

--- Adopt a module's account: every case ingested, red on any failed one, red on an empty one. A
--- leaf that may conduct holds the cargo lock: `locks = { prova.writes("cargo") }`.
--- ```lua
--- local cargo = require("prova.cargo")
--- prova.test("billing's unit cases pass", { locks = { prova.writes("cargo") } }, function(t)
---   cargo.verify(t)
--- end)
--- ```
---@param t prova.TestContext
---@param opts? { module?: string }
---@return prova.JunitReport
function cargo.verify(t, opts) end

return cargo
