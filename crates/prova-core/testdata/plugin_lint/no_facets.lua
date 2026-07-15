-- A library plugin: a plain table of helpers, no resource facets. Valid (not a resource, not an error).
local tokens = {}
function tokens.bearer(secret) return "Bearer " .. secret end
function tokens.basic(user, pass) return "Basic " .. user .. ":" .. pass end
return tokens
