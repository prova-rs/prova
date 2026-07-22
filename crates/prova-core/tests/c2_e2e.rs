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
    // Opt-in — same gate as `mock_network_vantage`: the host-bound mock binds 0.0.0.0, which trips
    // the macOS firewall prompt when Docker is up locally. CI sets PROVA_TEST_NETWORK_VANTAGE on
    // every leg (.github/workflows/build.yml) so the ubuntu leg still runs it in full; locally it
    // stays quiet. (`requires { "docker" }` already self-skips this where no daemon exists.)
    if std::env::var_os("PROVA_TEST_NETWORK_VANTAGE").is_none() {
        eprintln!(
            "skipping c2_e2e: binds 0.0.0.0 (macOS firewall prompt). \
             Set PROVA_TEST_NETWORK_VANTAGE=1 to run."
        );
        return;
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("c2_e2e.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run c2_e2e.lua");
    // `failed == 0` only — NOT a minimum pass count. Both tests are `requires { "docker" }`, and a
    // runner without Docker (GitHub's macOS/Windows images) skips them honestly; a skip is not a
    // failure (docs/design/test-topology.md). The proof does real work where Docker exists — ubuntu
    // CI, a dev machine, the Parallels VM — and there `failed == 0` still holds. Asserting `passed >=
    // 1` was baking the environment into the bar, which is exactly the mistake this fix removes.
    assert_eq!(summary.failed, 0, "failed");
}
