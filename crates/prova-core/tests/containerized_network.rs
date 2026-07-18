use std::path::PathBuf;

mod common;

// Defers to the engine's own capability probe, deliberately: this asserts pass/skip counts against
// what the engine decided, so if the two disagreed about "is docker available" the assertion would
// invert. One source of truth. (`docker info` alone is not it — Docker on Windows in
// Windows-container mode answers info and then cannot pull a linux image.)
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
}

/// The resource-vantage proof: a `prova.containerized` resource joined to a network with an alias
/// exposes a second addressing vantage — `resource.network = { url, host, port, alias }` (the alias
/// + container port an in-network SUT uses) alongside the host vantage (the mapped port the test
/// runner uses). The real bar is a sibling container reaching the resource via `network.host`/`port`
/// over embedded DNS. Runs where docker is present, skips (never fails) where it is absent.
#[test]
fn containerized_network_vantage_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/containerized_network.lua");
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
