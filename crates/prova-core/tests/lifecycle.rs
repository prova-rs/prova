use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// Runs the real design-showcase example end to end: three fixture scopes (suite/file/test),
/// fixture-to-fixture dependencies, lazy caching, and LIFO / inner-scope-first teardown.
#[test]
fn runs_the_lifecycle_poc_example() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/lifecycle_poc_test.lua");
    assert!(path.exists(), "example not found at {}", path.display());

    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run lifecycle_poc_test.lua");

    // All three tests pass: the test-scoped `conn` is rebuilt per test (each sees count 1),
    // the file-scoped `db` and suite-scoped `suite_dir` are cached and shared.
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.skipped, 0, "skipped");
}
