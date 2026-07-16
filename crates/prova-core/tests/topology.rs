use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

fn testdata(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(file)
}

/// A `prova.topology` is usable in test mode exactly like a fixture (`t:use`), and defaults to
/// `Scope.File` — provisioned once and shared across the file's tests. Both tests pass, and the
/// second observing `built == 1` proves the single shared instantiation. (`prova up`'s held-execution
/// path is exercised manually / in CLI smoke tests, since it blocks on a signal.)
#[test]
fn topology_is_usable_as_a_file_scoped_fixture() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("topology.lua"), &mut reporter).expect("run topology.lua");
    assert_eq!(summary.passed, 2, "both topology tests pass");
    assert_eq!(summary.failed, 0, "no failures");
}
