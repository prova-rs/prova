use std::path::PathBuf;
use std::process::{Command, Stdio};

use prova_core::{run_path_with, NullReporter, RunConfig};

fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The graduated `rust_cli` aspirational scenario, CWD-independently: render a local Lua archetype,
/// assert its layout under `prova.describe` (with soft assertions), and `cargo build` the output.
/// The build step is `requires`-gated on cargo, so it passes where cargo is present and skips where
/// it is absent — never fails. Mirrors `examples/rust_cli_test.lua` (which is CLI-verified from the
/// repo root); here the archetype path is baked in absolutely so the test runs from any directory.
#[test]
fn rust_cli_archetype_renders_and_builds_or_skips() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/fixtures/rust-cli")
        .canonicalize()
        .expect("rust-cli archetype fixture exists");

    // Build the test file via replacement (no `format!` brace-escaping against Lua's own braces).
    let test_lua = TEMPLATE.replace("__SRC__", &src.display().to_string());

    let dir = std::env::temp_dir().join(format!("prova-rust-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rust_cli_test.lua");
    std::fs::write(&path, test_lua).unwrap();

    let config = RunConfig::new(1).with_module(prova_archetect::install);
    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &config).expect("run rust_cli_test.lua");

    assert_eq!(summary.failed, 0, "never fails, cargo present or not");
    if cargo_available() {
        assert_eq!(summary.passed, 4, "all four tests pass with cargo present");
        assert_eq!(summary.skipped, 0, "nothing skips with cargo present");
    } else {
        assert_eq!(summary.passed, 3, "layout tests pass without cargo");
        assert_eq!(summary.skipped, 1, "the build test skips (requires cargo)");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

const TEMPLATE: &str = r#"
local src = [[__SRC__]]

local project = prova.fixture("project", Scope.File, function(ctx)
  return archetect.render{ source = src, answers = { project_name = "widget", description = "a demo cli" }, destination = ctx:tempdir(), defaults = true }
end)

prova.describe("rust-cli archetype", function()
  prova.test("produces the expected scaffold", function(t)
    local p = t:use(project)
    t:expect_all(function()
      t:expect(p:file("Cargo.toml")):exists()
      t:expect(p:file("src/main.rs")):exists()
      t:expect(p:file("README.md")):exists()
      t:expect(p:file(".gitignore")):exists()
    end)
  end)

  prova.test("wires the crate name through templates", function(t)
    t:expect(t:use(project):file("Cargo.toml"):read()):contains('name = "widget"')
  end)

  prova.test("has no leftover template markers", function(t)
    t:expect(t:use(project)):is_fully_rendered()
  end)

  prova.test("compiles cleanly", { timeout = "180s", requires = { "cargo" } }, function(t)
    local p = t:use(project)
    local r = shell.run("cargo build", { cwd = p.path, timeout = "180s" })
    t:expect(r.code):equals(0)
    t:expect(r.stderr):never():contains("error[")
  end)
end)
"#;
