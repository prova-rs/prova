use std::path::PathBuf;

use prova_core::{discover_suites, run_suites, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name)
}

/// The keystone of the layout plan: `require()` resolves a project-local module rooted at the home.
///
/// RED today — `package.path` is not rooted at the discovered home, so `require("shared.fixtures")`
/// raises "module not found" and the test errors. Once require roots at the home, the module loads
/// in the suite's state, its `prova.fixture` registers there, and the returned handle drives `t:use`
/// — the whole cross-suite sharing model in one proof. Rooting at the *home* (not the cwd) matters:
/// prova exposes no ambient cwd, so this must pass regardless of where cargo runs it from.
#[test]
fn require_resolves_a_project_local_shared_module() {
    let root = testdata("layout_require");
    let suites = discover_suites(&root).expect("discover");
    let mut reporter = NullReporter;
    let summary = run_suites(&suites, &mut reporter, &RunConfig::new(1)).expect("run");
    assert_eq!(summary.failed, 0, "failed (require should resolve, not error)");
    assert_eq!(summary.passed, 1, "the require-based test runs and passes");
}
