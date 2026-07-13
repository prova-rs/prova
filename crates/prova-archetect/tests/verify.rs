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

/// `archetect.verify{...}` — the declarative archetype check (prova's answer to the pytest
/// `manifest.yaml`). One call registers layout + fully-rendered + build tests; the build is
/// `requires`-gated on cargo. Runs against the local, dependency-free rust-cli archetype so it is
/// CWD-independent and offline.
#[test]
fn verify_helper_registers_and_runs_standard_checks() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/fixtures/rust-cli")
        .canonicalize()
        .expect("rust-cli archetype fixture exists");

    let test_lua = TEMPLATE.replace("__SRC__", &src.display().to_string());

    let dir = std::env::temp_dir().join(format!("prova-verify-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("verify_test.lua");
    std::fs::write(&path, test_lua).unwrap();

    let config = RunConfig::new(1).with_module(prova_archetect::install);
    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &config).expect("run verify_test.lua");

    assert_eq!(summary.failed, 0, "never fails, cargo present or not");
    if cargo_available() {
        assert_eq!(summary.passed, 3, "layout + fully-rendered + build all pass");
        assert_eq!(summary.skipped, 0, "nothing skips with cargo present");
    } else {
        assert_eq!(summary.passed, 2, "layout + fully-rendered pass without cargo");
        assert_eq!(summary.skipped, 1, "the build check skips (requires cargo)");
    }
}

const TEMPLATE: &str = r#"
archetect.verify {
  name = "rust-cli",
  source = [[__SRC__]],
  answers = { project_name = "widget", description = "a demo cli" },
  expected_files = { "Cargo.toml", "src/main.rs", "README.md", ".gitignore" },
  requires = { "cargo" },
  build_steps = { "cargo build" },
}
"#;
