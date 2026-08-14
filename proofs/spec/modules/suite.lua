-- The MODULE surface's own guardrails — the capability namespaces (`shell`, `docker`, `http`)
-- rather than the declaration DSL that `spec/engine` covers. Today that is one rule, and it is
-- the same rule the unit surface has held since api-freeze: an option prova cannot honor is
-- refused, never dropped (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
suite.config{ name = "spec-modules" }
