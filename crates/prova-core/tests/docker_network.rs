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

/// The networked-topologies proof: a user-defined Docker network, a container joined to it with a
/// stable alias while staying dual-homed on a published host port, and — the real bar — a sibling
/// container reaching it by that alias over the network's embedded DNS. Where docker is reachable
/// the proof runs for real; where it is absent, `requires = { "docker" }` skips it. Either way,
/// nothing fails: graceful degradation.
#[test]
fn docker_network_proof_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker_network.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run docker_network.lua");

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(
            summary.passed, 1,
            "the network proof passes when docker is present"
        );
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(
            summary.skipped, 1,
            "the network proof skips (requires docker) when it is absent"
        );
        assert_eq!(summary.passed, 0);
    }
}
