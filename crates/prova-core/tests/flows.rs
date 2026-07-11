use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// Runs the flow showcase example: ordered steps sharing closure state, a `flow`-scoped fixture
/// shared across steps, and cascade-skip.
///
/// The tally itself proves cascade-skip: the flow's third step calls `error(...)`, so if it had
/// run it would count as a *second* failure with zero skips. Observing exactly one failure and one
/// skip means the failing second step correctly cascade-skipped the third instead of executing it.
#[test]
fn runs_the_flow_poc_example() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/flow_poc_test.lua");
    assert!(path.exists(), "example not found at {}", path.display());

    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run flow_poc_test.lua");

    // order lifecycle: 3 steps pass. cascade flow: 1 pass, 1 fail, 1 cascade-skip.
    assert_eq!(summary.passed, 4, "passed");
    assert_eq!(summary.failed, 1, "failed");
    assert_eq!(summary.skipped, 1, "skipped");
}
