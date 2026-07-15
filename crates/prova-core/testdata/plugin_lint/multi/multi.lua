-- A multi-file plugin: its entry requires a vendored sibling by canonical namespace.
local helpers = require("multi.helpers")
return { container = helpers.mk() }
