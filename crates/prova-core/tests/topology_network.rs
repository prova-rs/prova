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
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run topology_network.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.passed, 0);
    }
}
