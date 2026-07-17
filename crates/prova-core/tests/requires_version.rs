use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

mod common;

/// Version predicates on capabilities — `requires = { "dotnet >= 9" }`.
///
/// The gap this closes came from a real failure: a suite said `requires = { "dotnet" }`, the machine
/// had SDK 8.0.421, the project targets net9.0, so the gate said "available" and the test died on
/// NETSDK1045 instead of skipping. A bare name cannot express "dotnet, but 9".
///
/// The bar: a satisfied constraint runs, an unsatisfied one SKIPS (never fails), a bare name is
/// unchanged, an absent tool skips without the version probe blowing up, semver operators mean what
/// semver says, and a platform predicate short-circuits before any probe runs.
///
/// One test needs a docker daemon, so the counts are docker-dependent — the same shape the docker
/// suites use, because asserting a fixed count across environments is what made the ergonomics test
/// the last red leg of the matrix.
#[test]
fn requires_version_predicates() {
    let _docker = common::docker_guard();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/requires_version.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run requires_version.lua");

    // Nothing here ever *fails*: an unmet requirement is a skip, always. That is the invariant.
    assert_eq!(summary.failed, 0, "failed (an unmet requirement skips)");

    // Runs:  bare git · git>=1.0 · git>=1.0 (no spaces) · git<9999 · range · unix   (+ docker>=1.0)
    // Skips: bare missing · git>=9999 · missing>=1.0 · git<0.1 · windows>=10
    //
    // The `unix` test is a *run* only on unix; on Windows it skips (the `windows` platform predicate
    // carries no version, so `windows >= 10` skips there too — no compensating run). So Windows is one
    // fewer pass and one more skip. Baking the Unix count in was the same environment-in-the-bar
    // mistake the mock harnesses just fixed: the platform, like docker, has to be a term in the count.
    let docker = prova_core::docker_runs_linux_containers();
    let unix = cfg!(unix);
    let pass = if unix { 6 } else { 5 } + if docker { 1 } else { 0 };
    let skip = if unix { 5 } else { 6 } + if docker { 0 } else { 1 };
    assert_eq!(summary.passed, pass, "passed");
    assert_eq!(summary.skipped, skip, "skipped");
}
