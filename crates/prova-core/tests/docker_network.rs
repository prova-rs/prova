use std::path::PathBuf;

mod common;

// Defers to the engine's own capability probe, deliberately: this asserts pass/skip counts against
// what the engine decided, so if the two disagreed about "is docker available" the assertion would
// invert. One source of truth. (`docker info` alone is not it — Docker on Windows in
// Windows-container mode answers info and then cannot pull a linux image.)
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
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
    let summary = common::run_proof(&path);

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
