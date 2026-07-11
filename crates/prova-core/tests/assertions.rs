use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

fn run(file: &str) -> prova_core::Summary {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(file);
    let mut reporter = NullReporter;
    run_path(&path, &mut reporter).expect("run")
}

/// Every new matcher — deep table equality, is_falsy/matches/has_length/is_one_of, numeric
/// compares, is_empty — plus a passing `expect_all`, all green.
#[test]
fn all_matchers_pass() {
    let summary = run("assertions.lua");
    assert_eq!(summary.passed, 4, "passed");
    assert_eq!(summary.failed, 0, "failed");
}

/// `expect_all` collects failures instead of aborting on the first: the failing block still runs to
/// its end (set a flag), which a second, passing test confirms — 1 fail + 1 pass.
#[test]
fn expect_all_collects_without_early_abort() {
    let summary = run("assertions_soft.lua");
    assert_eq!(summary.passed, 1, "the flag-check test passes");
    assert_eq!(
        summary.failed, 1,
        "the soft block fails once, with both failures"
    );
}
