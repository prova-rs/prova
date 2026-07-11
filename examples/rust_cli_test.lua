--- Example: acceptance test for the archetype-rust-cli archetype.
--- Renders it in-process, asserts on the layout, then actually builds the output.
---
--- Demonstrates: file-scoped fixtures (render once, assert many), fixture-to-fixture
--- dependency, soft assertions, and shelling out against the rendered project.

-- A scratch workspace, one per file. Auto-cleaned when the file's tests finish.
local workspace = assay.fixture("workspace", "file", function(ctx)
  return ctx:tempdir()
end)

-- Render the archetype once for the whole file. Every test below shares this output.
local project = assay.fixture("project", "file", function(ctx)
  return archetect.render{
    source = "https://github.com/archetect/archetype-rust-cli.git",
    answers = { project_name = "widget", description = "a demo cli" },
    destination = ctx:use(workspace),
    defaults = true,
  }
end)

assay.describe("archetype-rust-cli", function()
  assay.test("produces the expected scaffold", function(t)
    local p = t:use(project)
    -- Soft assertions: report every missing file, not just the first.
    t.expect_all(function()
      t.expect(p:file("Cargo.toml")):exists()
      t.expect(p:file("src/main.rs")):exists()
      t.expect(p:file("README.md")):exists()
      t.expect(p:file(".gitignore")):exists()
    end)
  end)

  assay.test("wires the crate name through templates", function(t)
    local cargo = t:use(project):file("Cargo.toml"):read()
    t.expect(cargo):contains('name = "widget"')
  end)

  assay.test("has no leftover template markers", function(t)
    local main = t:use(project):file("src/main.rs"):read()
    t.expect(main):never():contains("{{")
  end)

  assay.test("compiles cleanly", { timeout = "180s", tags = { "build" } }, function(t)
    local p = t:use(project)
    local r = shell.run("cargo build", { cwd = p.path, timeout = "180s" })
    t.expect(r.code):equals(0)
    t.expect(r.stderr):never():contains("error[")
  end)
end)
