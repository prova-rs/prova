use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

mod common;

// Defers to the engine's own capability probe, deliberately: this asserts pass/skip counts against
// what the engine decided, so if the two disagreed about "is docker available" the assertion would
// invert. One source of truth. (`docker info` alone is not it — Docker on Windows in
// Windows-container mode answers info and then cannot pull a linux image.)
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
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
