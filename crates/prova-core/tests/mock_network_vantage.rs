use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// C2's *mechanism* — provable on any host. The end-to-end reachability claim (a containerized SUT
/// reaching a host-bound mock, with 127.0.0.1 failing) needs native Linux and lives in the Parallels
/// harness; on Docker Desktop a loopback bind is reachable, so that mutation check cannot fail here.
///
/// What this pins is the wiring the Linux proof depends on: the vantage appears only when asked and
/// reports the host-gateway address, the mock actually binds beyond loopback (reached via the host's
/// own routable IP, with a loopback-only negative control), and `docker.run{ extra_hosts }` lands the
/// mapping in a real container's /etc/hosts. Two probes skip on a host with no non-loopback IPv4.
#[test]
fn mock_network_vantage_wiring() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("mock_network_vantage.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run mock_network_vantage.lua");
    // `failed == 0` only. Most checks here are host-independent, but two probe the machine's routable
    // IP and one needs Docker — all of which skip cleanly on a runner that can't answer. Asserting a
    // minimum pass count assumed an environment (a routable IP, a daemon) that CI images do not all
    // provide, and turned an honest skip into a red build.
    assert_eq!(summary.failed, 0, "failed");
}
