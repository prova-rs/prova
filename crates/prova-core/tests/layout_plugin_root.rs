use std::path::PathBuf;

use prova_core::{discover_suites, run_suites, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name)
}

/// A project's local plugins (`<root>/.prova/plugins/<name>/`) resolve against the project ROOT, not
/// the cwd — the "shared is a plugin" foundation. RED today: the disk searcher's `.prova/plugins`
/// root is cwd-relative, and cargo's cwd is the crate dir, so `require("greet")` fails. Once it roots
/// at `project_root`, it resolves regardless of where prova ran.
#[test]
fn local_plugins_resolve_against_the_project_root() {
    let root = testdata("layout_plugin_root");
    let suites = discover_suites(&root.join("proofs")).expect("discover");
    let mut reporter = NullReporter;
    let config = RunConfig::new(1).with_project(&root, &root);
    let summary = run_suites(&suites, &mut reporter, &config).expect("run");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 1, "the local .prova/plugins plugin resolves");
}
