use std::path::PathBuf;

mod common;

// Defers to the engine's own capability probe, deliberately: this asserts pass/skip counts against
// what the engine decided, so if the two disagreed about "is docker available" the assertion would
// invert. One source of truth. (`docker info` alone is not it — Docker on Windows in
// Windows-container mode answers info and then cannot pull a linux image.)
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
}

/// The containerized-SUT proof — the payoff of the networked-topology arc. A `prova.containerized`
/// recipe given `build` (rather than `image`) builds the system under test from its own Dockerfile,
/// runs it on the topology's ambient network wired to a resource's NETWORK vantage, and publishes its
/// own port so the host test runner drives it black-box over HTTP. The bar is end-to-end: the runner
/// probes the SUT over HTTP, the SUT answers with data it could only get by resolving the database by
/// DNS alias on the network, and mutations made through the DB's HOST vantage show up in the SUT's
/// answers — both vantages addressing one live resource. The suite needs nothing but Docker: no host
/// SDK, no toolchain. Runs where docker is present, skips (never fails) where it is absent.
#[test]
fn container_app_proof_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/container_app.lua");
    let summary = common::run_proof(&path);

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.passed, 0);
    }
}
