---@meta prova.workspace
--- `prova.workspace` — a scratch directory tied to a test/fixture scope.
--- `local workspace = require("prova.workspace")`. A bundled first-party plugin composing `fs` +
--- `ctx:manage`: the directory is created on demand and removed on scope teardown, so a proof that
--- renders/builds into it cleans up after itself without a `defer`.

--- A managed scratch directory. All relative paths resolve against `path`; teardown (via the
--- owning scope) removes the whole tree.
---@class prova.Workspace
---@field path string  # the workspace's absolute root directory
local Workspace = {}

--- Absolute path of a file inside the workspace (no I/O — just the join).
---@param rel string
---@return string
function Workspace:file(rel) end

--- Write `contents` to a relative path (parent directories are created) and return the absolute path.
---@param rel string
---@param contents string
---@return string
function Workspace:write(rel, contents) end

--- Read a relative path's contents.
---@param rel string
---@return string
function Workspace:read(rel) end

--- Whether a relative path exists.
---@param rel string
---@return boolean
function Workspace:exists(rel) end

--- Remove the workspace tree now (teardown calls this automatically at scope end).
function Workspace:close() end

---@class prova.workspace
local workspace = {}

--- Create a scratch directory whose lifetime is tied to `ctx` — removed on scope teardown.
---@param ctx prova.Context  # the fixture/test context (needs `:manage`)
---@param opts? table        # reserved for future options
---@return prova.Workspace
function workspace.create(ctx, opts) end

return workspace
