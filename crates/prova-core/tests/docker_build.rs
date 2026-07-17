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
