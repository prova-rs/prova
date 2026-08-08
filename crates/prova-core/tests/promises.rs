use std::path::PathBuf;

use prova_core::{run_path, run_path_with, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// The `promises` lifecycle (test-level only): an open promise (red body) is its own outcome, never
/// a failure; a promise that passes FAILS demanding graduation to `proves`; an unmet `requires`
/// still skips. Tallies match testdata/promises.lua.
#[test]
fn promise_outcomes_tally() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("promises.lua"), &mut reporter).expect("run promises.lua");
    assert_eq!(summary.promised, 2, "open promises: assertion, raise");
    assert_eq!(summary.failed, 1, "the honored promise demanding graduation");
    assert_eq!(summary.passed, 1, "the ordinary test holds the line");
    assert_eq!(summary.skipped, 1, "requires wins over an open promise");
    assert!(
        !summary.is_success(),
        "an honored-but-still-flagged promise fails the run"
    );
}

/// Open promises alone do not fail a run — CI stays green while contracts are authored ahead of
/// implementation.
#[test]
fn open_promises_do_not_fail_the_run() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("promises_open_only.lua"), &mut reporter)
        .expect("run promises_open_only.lua");
    assert_eq!(summary.promised, 1);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
    assert!(summary.is_success(), "open promises are not failures");
}

/// `--due` (driver mode): open promises ARE failures — the implementing agent's loop sees full red.
#[test]
fn due_turns_open_promises_into_failures() {
    let mut reporter = NullReporter;
    let config = RunConfig::default().with_due(true);
    let summary = run_path_with(&testdata("promises_open_only.lua"), &mut reporter, &config)
        .expect("run promises_open_only.lua due");
    assert_eq!(summary.promised, 0, "no open-promise outcomes in due mode");
    assert_eq!(summary.failed, 1, "the open promise is a real failure");
    assert!(!summary.is_success());
}

/// `--promises` (the selector): run exactly the leaves carrying a `promises` flag — unflagged tests
/// are deselected. Green promised leaves still fail demanding graduation.
#[test]
fn promises_selector_narrows_to_the_open_surface() {
    let mut reporter = NullReporter;
    let config = RunConfig::default().with_promises_only(true);
    let summary =
        run_path_with(&testdata("promises.lua"), &mut reporter, &config).expect("run promises-only");
    // Selected: 2 open + 1 honored + 1 requires-skip. Deselected: the ordinary test.
    assert_eq!(summary.promised, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.passed, 0, "unflagged tests are not run");
    assert_eq!(summary.deselected, 1, "the ordinary test is deselected");
}

/// The `promises` flag is test-level only: a group-level flag is refused with the fix (flag each
/// open test), never silently inherited.
#[test]
fn group_level_promises_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("promises_group.lua"), &mut reporter)
        .expect_err("a group-level promises flag must refuse to run");
    let msg = err.to_string();
    assert!(
        msg.contains("promises is test-level only"),
        "states the rule: {msg}"
    );
    assert!(msg.contains("formats"), "names the offending group: {msg}");
}
