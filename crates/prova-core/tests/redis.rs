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

/// The `redis` client + `redis.container` recipe against a REAL Redis in an ephemeral container.
/// Runs for real where docker is present, skips (via `requires`) where it is absent.
#[test]
fn redis_client_and_recipe_or_skips() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/redis_test.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run redis_test.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 3, "the three redis tests pass with docker");
    } else {
        assert_eq!(summary.skipped, 3, "skip (requires docker) when absent");
    }
}
