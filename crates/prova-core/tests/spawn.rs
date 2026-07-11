use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `shell.spawn` process lifecycle: start a long-running process, observe it running, stop it
/// (async, idempotent), and read a finished process's exit code via wait().
#[test]
fn spawn_manages_process_lifecycle() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/spawn.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run spawn.lua");
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
