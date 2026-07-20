-- A provider plugin that a *library* depends on privately. Its whole reason to exist is the isolation
-- proof: a library may compose it internally, but a consumer that only required the library must never
-- be able to reach it. The surface is deliberately tiny — one value + one helper — so a proof can show
-- both halves: "the library reached inner" and "the consumer cannot".
local M = {}

M.secret = "inner-secret"

function M.stamp(s)
  return s .. "::stamped-by-inner"
end

return M
