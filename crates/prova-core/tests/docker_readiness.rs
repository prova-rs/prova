use std::path::PathBuf;

mod common;

// Defers to the engine's own capability probe, deliberately: this asserts pass/skip counts against
// what the engine decided, so if the two disagreed about "is docker available" the assertion would
// invert. One source of truth. (`docker info` alone is not it — Docker on Windows in
// Windows-container mode answers info and then cannot pull a linux image.)
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
}

/// The readiness contract: when `docker.run` returns, the container is READY — a client's FIRST probe
/// succeeds, with no retry. `wait` offers signals honest about different observables, and the proof
/// exercises each on a server where it is TRUE: `wait = { port }` asks the container's own kernel
/// (`/proc/net/tcp`) what is listening (not the mapped host port, which Docker Desktop's proxy
/// accepts the moment the container starts — passing while the server boots and never failing for a
/// container that never listens); `wait = { cmd }` runs a real readiness command for a server whose
/// socket predates its serving (Postgres, where a `port` probe races "the database system is
/// starting up"). The bar has four parts — the port probe's first probe succeeds; the cmd probe's
/// first query succeeds where a port probe would race; an UNPUBLISHED port (an in-network-only
/// resource) is still waitable; and a container that never listens times out rather than being waved
/// through. Runs where docker is present, skips (never fails) where it is absent.
#[test]
fn docker_readiness_proof_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker_readiness.lua");
    let summary = common::run_proof(&path);

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 4);
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(summary.skipped, 4);
        assert_eq!(summary.passed, 0);
    }
}
