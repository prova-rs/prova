use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `prova.containerized` builds a grammar-conformant namespace from a spec (the scaffolding helper
/// every recipe/plugin is authored through). Shape-only — no docker needed.
#[test]
fn containerized_builds_conformant_namespace() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("containerized.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run containerized.lua");
    assert_eq!(summary.passed, 6, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
