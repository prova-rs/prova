use std::path::PathBuf;

use prova_core::{discover_suites, run_suites, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// A directory with a `suite.lua` is one suite whose files share a Lua state: a `Scope.Suite` fixture
/// is built **once** and shared across the files (cross-file sharing — the whole point), while a
/// `Scope.File` fixture rebuilds per file. Four tests across two files, all green.
#[test]
fn suite_shares_suite_scope_across_files() {
    let suites = discover_suites(&testdata("suite_shared")).expect("discover");
    assert_eq!(suites.len(), 1, "a suite.lua dir is a single suite");
    assert_eq!(suites[0].files.len(), 2, "both test files are members");
    assert!(suites[0].setup.is_some(), "suite.lua is the setup");

    let mut reporter = NullReporter;
    let summary = run_suites(&suites, &mut reporter, &RunConfig::new(1)).expect("run suite");
    assert_eq!(summary.passed, 4, "all four tests pass");
    assert_eq!(summary.failed, 0, "failed");
}

/// A directory of ungrouped `*_test.lua` (no `suite.lua`) yields one singleton suite per file — the
/// backward-compatible behaviour.
#[test]
fn ungrouped_files_are_singleton_suites() {
    let suites = discover_suites(&testdata("suite")).expect("discover");
    assert!(
        suites.iter().all(|s| s.setup.is_none() && s.files.len() == 1),
        "each ungrouped file is its own one-file suite: {:?}",
        suites.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}
