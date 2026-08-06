-- Quality gate: clippy is clean at -D warnings across the whole workspace. This is prova holding
-- its own Rust to the same bar it will hold others' code to — findings as a red condition, one
-- runner, one wall. It shells the exact flags `cargo xtask clippy` uses (minus the artifact sweep),
-- so the proof and the xtask stay in lockstep.
--
-- HEAVY: clippy recompiles the workspace (~20s), so this must never fire because a person typed
-- `prova`. It `requires` the `quality` capability (.prova/config.lua, gated on PROVA_QUALITY), which
-- the `quality` profile turns on. Plain local runs skip it; `prova --profile quality` and CI run it.

prova.test("clippy is clean (-D warnings, whole workspace)", { requires = { "quality" } }, function(t)
  local r = shell.run(
    { "cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings" },
    { cwd = prova.root, merge_stderr = true }
  )
  t:expect(r.code, "clippy reported warnings or errors:\n" .. (r.stdout or "")):equals(0)
end)
