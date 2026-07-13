use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `net.free_port()` returns a plausible ephemeral port, repeatably.
#[test]
fn net_free_port_returns_valid_ports() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("net.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run net.lua");
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
