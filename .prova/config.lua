-- The runtime companion: loaded once, pre-suite. `runtime.*` is only valid here.
-- A marker capability so a proof can demonstrate the companion is wired.
runtime.capability("prova_selftest", function() return true end)

-- `soak` — the OPT-IN gate on the long-running container-runtime soaks under proofs/soak/.
--
-- It means exactly one thing: "someone asked for a soak". Soaks take minutes to hours and hammer
-- the container runtime, so they must never happen because a person typed `prova`.
--
-- It deliberately does NOT also check for docker. A soak proof asks for both — `requires = { "soak",
-- "docker" }` — because those are two separate facts with two separate remedies: one is fixed by
-- setting the variable, the other by installing a runtime. Folding them into one predicate would
-- report "soak unavailable" for a machine that simply has no docker, and a capability that can be
-- false for two unrelated reasons cannot tell you which.
--
-- A capability rather than a tag because this is what capabilities already mean: `requires` skips
-- gracefully where something is unavailable, which is the wanted behaviour, and needs no new
-- selection flags at the call site.
runtime.capability("soak", function()
  return os.getenv("PROVA_SOAK") ~= nil
end)

-- `quality` — the OPT-IN gate on the heavy, cargo-based code-quality proofs under proofs/quality/
-- (the clippy gate, the unwrap/expect census). Each recompiles the workspace, so — exactly like
-- `soak` — they must never happen because a person typed `prova`. The `quality` profile switches
-- this on (its env sets PROVA_QUALITY) and `must_run`s it so it can't silently skip once selected;
-- a plain `prova` skips them. The fast file-size gate in the same directory needs no capability.
runtime.capability("quality", function()
  return os.getenv("PROVA_QUALITY") ~= nil
end)

-- There is no `placement_broker` gate anymore. The placement conformance suite
-- (proofs/spec/placement/) is hermetic: with no PROVA_PLACEMENT_BROKER named it spawns the MIT
-- reference broker (`prova broker`, through prova.bin) per test, so the suite runs — and the
-- spec's promises stay kept — on any unix machine. Naming an address still points the same suite
-- at an external broker, which is how a third-party implementation proves itself.
