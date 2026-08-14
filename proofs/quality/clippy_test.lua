-- Quality gate: clippy is clean at -D warnings across the whole workspace. This is prova holding
-- its own Rust to the same bar it will hold others' code to — findings as a red condition, one
-- runner, one wall. It shells the canonical clippy flags (-D warnings, all targets/features),
-- so the proof and the xtask stay in lockstep.
--
-- HEAVY: clippy recompiles the workspace (~20s), so this must never fire because a person typed
-- `prova`. It sits behind the `quality` switch — off unless thrown (docs/design/manifest.md
-- #switches-not-env-capabilities). `prova run quality` throws it; `prova -s quality` is the
-- ad-hoc door. Plain local runs hold it back, reported on the switched-off summary line.

prova.test("clippy is clean (-D warnings, whole workspace)", {
  switch = "quality",
  -- The house rule this repo bled for: cargo takes process-wide locks of its own, so two prova
  -- instances that both reach for it (a bank + a sweep) contend unpredictably unless the rule
  -- is said out loud. Held across instances (prova learn locks).
  locks = { prova.writes("cargo") },
}, function(t)
  -- Say which clippy answered. A gate is only as good as the tool behind it, and this one spent a
  -- day passing locally while failing on main because the two ran different versions
  -- (docs/design/agent-ergonomics.md#local-clippy-weaker-than-ci). `rust-toolchain.toml` pins
  -- them together; logging the version is what makes a future divergence visible in the output
  -- instead of on main.
  local v = shell.run({ "cargo", "clippy", "--version" }, { cwd = prova.root, merge_stderr = true })
  t:log("clippy: " .. (v.stdout or ""):gsub("%s+$", ""))

  local r = shell.run(
    { "cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings" },
    { cwd = prova.root, merge_stderr = true }
  )
  t:expect(r.code, "clippy reported warnings or errors:\n" .. (r.stdout or "")):equals(0)
end)

prova.test("the toolchain is pinned, so local and CI lint identically", {
  switch = "quality",
  covers = "docs/design/agent-ergonomics.md#local-clippy-weaker-than-ci",
  proves = "the pin IS the fix — without the file, rustup falls back to whatever a machine happens to default to, and the gate silently goes back to answering for a different compiler than the one CI runs",
}, function(t)
  local pinned = fs.read(prova.root .. "/rust-toolchain.toml")
  local channel = pinned:match('channel%s*=%s*"([^"]+)"')
  t:expect(channel, "a channel is pinned"):never():is_nil()
  -- An exact version, not a moving channel: `stable` would reintroduce the divergence on a
  -- six-week timer, with nothing in the diff to explain the new red.
  t:expect(channel, "…to an exact version rather than a moving channel"):matches("^%d+%.%d+")
  t:expect(pinned, "…with clippy present, since the gate depends on it"):contains("clippy")

  -- And the toolchain actually in use is that one — a pin nothing honors is decoration. `rustc`
  -- rather than `clippy --version`, which reports its own 0.1.x line and would match the pin only
  -- by coincidence.
  local v = shell.run({ "rustc", "--version" }, { cwd = prova.root, merge_stderr = true })
  t:expect(v.code, "rustc runs"):equals(0)
  t:expect(v.stdout, "the running toolchain is the pinned one: " .. v.stdout):contains(channel)
end)
