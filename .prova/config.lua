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

-- `placement_broker` — the gate on the placement conformance suite (proofs/spec/placement/,
-- docs/design/placement.md). It means exactly one thing: "an address to prove against was named".
--
-- It deliberately does NOT dial the socket to check it answers. A configured-but-unreachable broker
-- is a LOUD failure in the spec's own terms, not an absence: silently skipping it would report the
-- same green as a machine that never had a broker, which is the difference the suite exists to
-- police. Reachability is the proof's job; presence of an address is this predicate's.
--
-- Returns a boolean, not a message. A capability predicate that returns a string is read as a
-- VERSION, so a helpful explanation turns a clean skip into a hard configuration error.
runtime.capability("placement_broker", function()
  return os.getenv("PROVA_PLACEMENT_BROKER") ~= nil
end)
