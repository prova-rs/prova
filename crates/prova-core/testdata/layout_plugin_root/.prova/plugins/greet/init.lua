-- A local plugin under the project's .prova/plugins/. Reached as require("greet") — the "shared is
-- a plugin" mechanism. Found via the project ROOT, regardless of where prova was invoked from.
local greeter = prova.fixture("greeter", Scope.Test, function() return "from a local plugin" end)
return { greeter = greeter }
