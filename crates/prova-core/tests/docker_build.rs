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

/// The image-build primitive proof: `docker.build{ context, dockerfile?, tag?, buildargs? }` turns a
/// Dockerfile into a real local image that `docker.run{ image = … }` then runs like any pulled one.
/// The bar is that the built image RUNS and carries what the Dockerfile put in it (the context was
/// sent; build args were applied), that a nested dockerfile path resolves `COPY` against the context
/// root, and that a failing build RAISES with the builder's own log rather than handing back a ref to
/// an image that does not exist. Runs where docker is present, skips (never fails) where it is absent.
#[test]
fn docker_build_proof_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker_build.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run docker_build.lua");

    assert_eq!(summary.failed, 0, "never fails, docker present or not");
    if docker_available() {
        assert_eq!(summary.passed, 3);
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(summary.skipped, 3);
        assert_eq!(summary.passed, 0);
    }
}
