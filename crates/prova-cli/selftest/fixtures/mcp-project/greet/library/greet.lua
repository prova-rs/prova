---@meta greet
--- `greet` — the fixture plugin the self-tests introspect.

---@class greet
local greet = {}

--- Compose a greeting for `name` (defaults to "world").
---@param name? string
---@return string
function greet.hello(name) end

return greet
