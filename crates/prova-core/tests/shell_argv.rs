use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `shell.run`/`shell.spawn` accept an **argv table** (no shell, no quoting) as well as a shell
/// string — closing the asymmetry with `container:run`, which had argv all along. The injection
/// case is the one that matters: content passed through argv is data, never commands.
/// See docs/design/agent-ergonomics.md §1.
#[cfg_attr(windows, ignore = "the Lua suite drives unix tools (echo/sleep binaries, \n endings)")]
#[test]
fn shell_accepts_argv_as_well_as_a_shell_string() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("shell_argv.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run");
    assert_eq!(summary.failed, 0, "argv suite had failures: {summary:?}");
    assert!(
        summary.passed >= 5,
        "expected the whole argv suite to run, got {summary:?}"
    );
}
