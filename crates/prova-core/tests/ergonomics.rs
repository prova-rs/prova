use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `prova.retry` (returns on truthy / non-raising, times out with a message) and `ctx:manage`
/// (returns the resource, stops or closes it at scope end, rejects an unmanageable one).
#[test]
fn retry_and_manage_ergonomics() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("ergonomics.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run ergonomics.lua");
    assert_eq!(summary.passed, 12, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
