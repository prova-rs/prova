use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `prova.retry` (returns on truthy / non-raising, times out with a message) and `ctx:manage`
/// (returns the resource, stops or closes it at scope end, rejects an unmanageable one).
///
/// Three of the twelve drive a shell STRING with POSIX syntax, which `cmd /C` cannot run, so they
/// carry `requires = { "unix" }` and skip off unix. Asserting an exact `passed` count on both
/// platforms is what made this the last red leg of the matrix: the count is platform-dependent, so
/// the assertion has to be too — the same shape the docker tests use. `failed == 0` everywhere is
/// the part that is actually universal.
#[test]
fn retry_and_manage_ergonomics() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("ergonomics.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run ergonomics.lua");
    assert_eq!(summary.failed, 0, "failed");
    if cfg!(unix) {
        assert_eq!(summary.passed, 12, "passed");
        assert_eq!(summary.skipped, 0, "skipped");
    } else {
        assert_eq!(summary.passed, 9, "passed (3 posix-shell tests skip)");
        assert_eq!(summary.skipped, 3, "skipped");
    }
}
