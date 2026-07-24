-- THE SPEC for the plugin registry (docs/design/registry.md) — discovery across config-listed
-- registries: `prova plugins` list/search/info/add, per-entry schema tolerance, the built-in
-- default + user merge, and the discovery-only line (`require` never consults a registry).
-- Spec flags are test-level (each open test carries its own); this file only names the suite.
suite.config{ name = "spec-registry" }
