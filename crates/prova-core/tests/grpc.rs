use std::path::PathBuf;
use std::process::{Command, Stdio};

use prova_core::{run_path, NullReporter};

mod common;

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The native `grpc` module against a REAL reflection-enabled server (moul/grpcbin) in an ephemeral
/// container — no `grpcurl` binary, no `.proto` files. Runs for real where docker is present, skips
/// (via `requires`) where it is absent. Either way, nothing fails.
#[test]
fn grpc_module_calls_real_server_or_skips() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/grpc_test.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run grpc_test.lua");

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(
            summary.passed, 3,
            "the three gRPC round-trips pass with docker present"
        );
    } else {
        assert_eq!(
            summary.skipped, 3,
            "skips (requires docker) when it is absent"
        );
    }
}
