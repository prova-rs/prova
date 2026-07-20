use std::path::PathBuf;

use prova_core::{discover_suites, run_suites, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// A declared plugin root resolves regardless of where prova was invoked from.
///
/// Cargo runs this with the crate dir as cwd, never the fixture root, so a root that resolved
/// relative to the working directory could not find `greet` here. Roots are now absolute and
/// declared — the CLI derives it from the manifest's `[run] plugin_root` against the project root,
/// and an embedder passes it directly, as below. Nothing is implied by the engine.
#[test]
fn local_plugins_resolve_against_the_project_root() {
    let root = testdata("layout_plugin_root");
    let suites = discover_suites(&root.join("proofs")).expect("discover");
    let mut reporter = NullReporter;
    let config = RunConfig::new(1)
        .with_project(&root)
        .with_plugin_root(root.join(".prova/plugins"));
    let summary = run_suites(&suites, &mut reporter, &config).expect("run");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(
        summary.passed, 1,
        "the local .prova/plugins plugin resolves"
    );
}
