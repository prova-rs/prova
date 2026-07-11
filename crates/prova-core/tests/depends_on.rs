use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// Runs the dependency-DAG showcase: `depends_on` edges over tests, flows, and groups.
///
/// As with the flow example, the tally proves cascade-skip: the three downstream units all call
/// `error(...)` in their bodies, so had any of them *run* it would be a failure, not a skip.
/// Observing exactly one failure (`boot service`) and three skips means the failed upstream
/// correctly cascade-skipped its entire transitive downstream — including through a group edge —
/// rather than executing any of it.
#[test]
fn runs_the_depends_on_example() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/depends_on_test.lua");
    assert!(path.exists(), "example not found at {}", path.display());

    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run depends_on_test.lua");

    // login(1) + populate(2) + checkout(1) + settings(1) = 5 pass; boot = 1 fail;
    // probe + downstream/consume + report = 3 cascade-skips.
    assert_eq!(summary.passed, 5, "passed");
    assert_eq!(summary.failed, 1, "failed");
    assert_eq!(summary.skipped, 3, "skipped");
}
