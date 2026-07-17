use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// C2 end-to-end: a containerized SUT reaching a host-bound mock via the network vantage.
///
/// The positive test passes on any Docker; the mutation ("a loopback-bound mock is unreachable from
/// the container") only *holds* on native Linux — on Docker Desktop the daemon's proxy forwards
/// host.docker.internal to host loopback, so it self-skips there (a claim about the environment, per
/// docs/design/test-topology.md). Hence `failed == 0` and `passed >= 1` rather than an exact count:
/// on a dev Mac it is 1 passed / 1 skipped; on native Linux (CI, or the Parallels harness) 2 passed.
///
/// The honest end-to-end proof — including the mutation running for real and the platform divergence
/// that makes it meaningful — is run by `scripts/vm-linux-proof.sh` inside a Linux VM.
#[test]
fn c2_containerized_sut_reaches_host_bound_mock() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("c2_e2e.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run c2_e2e.lua");
    assert_eq!(summary.failed, 0, "failed");
    assert!(summary.passed >= 1, "the positive reachability test must run where docker exists");
}
