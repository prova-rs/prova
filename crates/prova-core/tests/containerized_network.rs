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

/// The resource-vantage proof: a `prova.containerized` resource joined to a network with an alias
/// exposes a second addressing vantage — `resource.network = { url, host, port, alias }` (the alias
/// + container port an in-network SUT uses) alongside the host vantage (the mapped port the test
/// runner uses). The real bar is a sibling container reaching the resource via `network.host`/`port`
/// over embedded DNS. Runs where docker is present, skips (never fails) where it is absent.
#[test]
fn containerized_network_vantage_runs_or_skips_gracefully() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/containerized_network.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run containerized_network.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.passed, 0);
    }
}
