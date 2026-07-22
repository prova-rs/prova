use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

fn run(path: PathBuf) -> prova_core::Summary {
    assert!(path.exists(), "not found: {}", path.display());
    let mut reporter = NullReporter;
    run_path(&path, &mut reporter).expect("run")
}

/// A fixture factory that `await`s (here `prova.sleep`) — impossible before `ctx:use` became an
/// async method. Also proves the await chains through a fixture-depends-on-fixture edge.
#[test]
fn fixture_factories_can_await() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/async_fixture.lua");
    let summary = run(path);
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 0, "failed");
}

/// The `shell` + `fs` slice end to end: an async fixture builds a workspace via `shell.run`, then
/// tests assert on it with `shell` (exit/stdout), `fs` (exists/read/glob), and the filesystem
/// matchers. Unix-only: the example drives a POSIX shell (`sh -c`, `mkdir`, `cat`).
#[cfg(unix)]
#[test]
fn shell_and_fs_modules_drive_a_workspace() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/shell_fs.lua");
    let summary = run(path);
    assert_eq!(summary.passed, 5, "passed");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.skipped, 0, "skipped");
}
