-- THE SPEC for the utility belt (docs/plans/api-freeze.md §1): tiny native-required primitives —
-- base64, hash, uuid, url — that no pure-Lua plugin can provide for itself. Spec flags are
-- test-level (each open test carries its own); this file only names the suite.
suite.config{ name = "spec-utilities" }
