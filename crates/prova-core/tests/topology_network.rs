use std::path::PathBuf;

mod common;

// Defers to the engine's own capability probe, deliberately: this asserts pass/skip counts against
// what the engine decided, so if the two disagreed about "is docker available" the assertion would
// invert. One source of truth. (`docker info` alone is not it — Docker on Windows in
// Windows-container mode answers info and then cannot pull a linux image.)
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
}

/// The networked-topology convenience proof: a `prova.topology` factory exposes an ambient managed
/// docker network on `ctx.network` (created lazily, scope-managed), and a `prova.containerized`
/// resource provisioned in that factory auto-joins it aliased by its recipe name — no
/// `docker.network()` call and no `network`/`alias` opts authored. The real bar is a sibling probe
/// reaching the resource by its auto-alias over the auto-network. Runs where docker is present,
/// skips (never fails) where it is absent.
#[test]
fn topology_network_convenience_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/topology_network.lua");
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
