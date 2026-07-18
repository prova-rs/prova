use std::path::PathBuf;

mod common;

// Same single source of truth as the other docker proofs: the engine's own capability probe decides
// whether these run for real or skip.
fn docker_available() -> bool {
    prova_core::docker_runs_linux_containers()
}

/// The failure-reporting proof: a container that exits is diagnosed immediately (exit code + its own
/// logs) rather than waited out, while a container that is merely slow still gets its whole budget.
/// Where docker is reachable the proof runs for real; where it is absent it skips. Either way,
/// nothing fails.
#[test]
fn docker_failure_reporting_proof_runs_or_skips_gracefully() {
    let _docker = common::docker_guard();
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/docker_failure_reporting.lua");
    let summary = common::run_proof(&path);

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(
            summary.passed, 2,
            "both failure-reporting proofs pass when docker is present"
        );
        assert_eq!(summary.skipped, 0);
    } else {
        assert_eq!(
            summary.skipped, 2,
            "both proofs skip (require docker) when it is absent"
        );
        assert_eq!(summary.passed, 0);
    }
}
