--- The declarative archetype check — prova's answer to the pytest harness's `manifest.yaml`, matched
--- field-for-field but as real Lua you can extend. One call renders the archetype headlessly and
--- registers the standard tests: layout (expected/absent files), no leftover template markers, yaml
--- manifests parse, and a `requires`-gated build. Run from the repo root:
--- `prova examples/archetype_verify_test.lua`.
---
--- Compare to the pytest manifest.yaml this replaces — same fields, same brevity, but now you can
--- drop into full fixtures/containers alongside it when you need the runtime tier.

archetect.verify {
  name = "rust-cli",
  source = "examples/fixtures/rust-cli",             -- local Lua archetype (or a git URL)
  answers = { project_name = "widget", description = "a demo cli" },
  expected_files = { "Cargo.toml", "src/main.rs", "README.md", ".gitignore" },
  requires = { "cargo" },
  build_steps = { "cargo build" },
}
