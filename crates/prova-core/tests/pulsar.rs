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

/// The `pulsar` produce/consume client + `pulsar.container` recipe against a REAL Pulsar standalone.
/// Runs for real where docker is present (heavy image, slow start), skips (via `requires`) otherwise.
#[test]
fn pulsar_produce_consume_or_skips() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/pulsar_test.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run pulsar_test.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 1, "the produce/consume round-trip passes with docker");
    } else {
        assert_eq!(summary.skipped, 1, "skips (requires docker) when absent");
    }
}
