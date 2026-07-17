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

/// The `docker` module + `requires` gating together. Where docker is reachable, the two tests run
/// a real container (traefik/whoami), map a random host port, wait for readiness, and probe it over
/// HTTP. Where docker is absent, `requires = { "docker" }` skips them — and the container fixture,
/// being lazy, never starts. Either way, nothing fails: graceful degradation.
#[test]
fn docker_module_runs_a_container_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run docker.lua");

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(
            summary.passed, 4,
            "all container tests pass when docker is present"
        );
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(
            summary.skipped, 4,
            "all tests skip (requires docker) when it is absent"
        );
        assert_eq!(summary.passed, 0);
    }
}
