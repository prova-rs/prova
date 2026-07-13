--- Acceptance test for a Rust-CLI archetype: render it in-process, assert on the layout, then
--- actually `cargo build` the output. Run from the repo root: `prova examples/rust_cli_test.lua`.
---
--- Demonstrates: file-scoped fixtures (render once, assert many), fixture-to-fixture dependency,
--- `prova.describe` labeling, soft assertions, and shelling out against the rendered project. The
--- archetype is a local, dependency-free Lua archetype (under examples/fixtures/rust-cli) so the
--- build is offline and fast; the build step is `requires`-gated on cargo so it skips where absent.

-- A scratch workspace, one per file. Auto-cleaned when the file's tests finish.
local workspace = prova.fixture("workspace", Scope.File, function(ctx)
  return ctx:tempdir()
end)

-- Render the archetype once for the whole file. Every test below shares this output.
local project = prova.fixture("project", Scope.File, function(ctx)
  return archetect.render{
    source = "examples/fixtures/rust-cli",  -- local archetype, relative to CWD
    answers = { project_name = "widget", description = "a demo cli" },
    destination = ctx:use(workspace),
    defaults = true,
  }
end)

prova.describe("rust-cli archetype", function()
  prova.test("produces the expected scaffold", function(t)
    local p = t:use(project)
    -- Soft assertions: report every missing file, not just the first.
    t:expect_all(function()
      t:expect(p:file("Cargo.toml")):exists()
      t:expect(p:file("src/main.rs")):exists()
      t:expect(p:file("README.md")):exists()
      t:expect(p:file(".gitignore")):exists()
    end)
  end)

  prova.test("wires the crate name through templates", function(t)
    local cargo = t:use(project):file("Cargo.toml"):read()
    -- optional label → failure reads "Cargo.toml [package] name: expected to contain ..."
    t:expect(cargo, "Cargo.toml [package] name"):contains('name = "widget"')
  end)

  prova.test("has no leftover template markers anywhere", function(t)
    -- One call scans every file (contents + path segments) for unrendered jinja markers.
    t:expect(t:use(project)):is_fully_rendered()
  end)

  prova.test("compiles cleanly", { timeout = "180s", tags = { "build" }, requires = { "cargo" } }, function(t)
    local p = t:use(project)
    local r = shell.run("cargo build", { cwd = p.path, timeout = "180s" })
    t:expect(r.code):equals(0)
    t:expect(r.stderr):never():contains("error[")
  end)
end)
