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

/// The readiness contract: when `docker.run` returns, the container is READY — a client's FIRST probe
/// succeeds, with no retry. `wait = { port }` asks the container's own kernel (`/proc/net/tcp`) what
/// is listening, rather than connecting to the mapped host port, which is not a signal at all on
/// Docker Desktop: the port proxy accepts the moment the container starts, so the old check passed
/// while the server was still booting and NEVER failed for a container that never listened. The bar
/// has three parts — the first probe succeeds; an UNPUBLISHED port (an in-network-only resource) is
/// still waitable; and a container that never listens times out rather than being waved through.
/// Runs where docker is present, skips (never fails) where it is absent.
#[test]
fn docker_readiness_proof_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker_readiness.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run docker_readiness.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 3);
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(summary.skipped, 3);
        assert_eq!(summary.passed, 0);
    }
}
