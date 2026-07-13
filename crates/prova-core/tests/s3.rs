use std::path::PathBuf;
use std::process::{Command, Stdio};

use prova_core::{run_path, NullReporter};

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The `s3` object-storage client + `s3.container` recipe against a REAL MinIO. Runs for real where
/// docker is present, skips (via `requires`) otherwise.
#[test]
fn s3_object_storage_or_skips() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/s3_test.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run s3_test.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 2, "the two object-storage tests pass with docker");
    } else {
        assert_eq!(summary.skipped, 2, "skip (requires docker) when absent");
    }
}
