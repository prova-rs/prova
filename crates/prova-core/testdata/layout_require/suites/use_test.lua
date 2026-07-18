-- The keystone: a test requires a project-local module by module path. Today `package.path` is not
-- rooted at the home, so this raises "module 'shared.fixtures' not found" and the test errors — RED.
-- Once require roots at the home, the module resolves, its fixture registers in this state, and the
-- returned handle drives `t:use`.
local F = require("shared.fixtures")

prova.test("requires a project-local shared module and uses its fixture handle", function(t)
  t:expect(t:use(F.answer)):equals(42)
end)
