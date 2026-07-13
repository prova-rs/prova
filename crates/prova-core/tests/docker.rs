use std::path::PathBuf;
use std::process::{Command, Stdio};

use prova_core::{run_path, NullReporter};

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The `docker` module + `requires` gating together. Where docker is reachable, the two tests run
/// a real container (traefik/whoami), map a random host port, wait for readiness, and probe it over
/// HTTP. Where docker is absent, `requires = { "docker" }` skips them — and the container fixture,
/// being lazy, never starts. Either way, nothing fails: graceful degradation.
#[test]
fn docker_module_runs_a_container_or_skips_gracefully() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run docker.lua");

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(
            summary.passed, 3,
            "all container tests pass when docker is present"
        );
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(
            summary.skipped, 3,
            "all tests skip (requires docker) when it is absent"
        );
        assert_eq!(summary.passed, 0);
    }
}
