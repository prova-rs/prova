-- The runtime companion: loaded once, pre-suite. `runtime.*` is only valid here.
-- A marker capability so a proof can demonstrate the companion is wired.
runtime.capability("prova_selftest", function() return true end)

-- The opt-in test classes (soak / quality / ut) used to be registered here as env-var-gated
-- capabilities. They are `switch = "<class>"` declarations on the proofs themselves now —
-- fail-closed at the declaration site, thrown by `-s <class>` or a profile's `switches`, with
-- `requires` back to meaning world facts only (docker, cargo-nextest). See
-- docs/design/manifest.md#switches-not-env-capabilities for why intent is a selection fact,
-- never a capability.

-- There is no `placement_broker` gate anymore. The placement conformance suite
-- (proofs/spec/placement/) is hermetic: with no PROVA_PLACEMENT_BROKER named it spawns the MIT
-- reference broker (`prova broker`, through prova.bin) per test, so the suite runs — and the
-- spec's promises stay kept — on any unix machine. Naming an address still points the same suite
-- at an external broker, which is how a third-party implementation proves itself.
