use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

#[test]
fn runs_and_tallies_outcomes() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/smoke.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run smoke.lua");

    // 2 passing tests, 1 intentional failure, 1 explicit skip.
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 1, "failed");
    assert_eq!(summary.skipped, 1, "skipped");
    assert!(!summary.is_success());
}
