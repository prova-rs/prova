use std::path::PathBuf;

use prova_core::{run_path, run_path_with, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// The `spec` lifecycle (docs/plans/api-freeze.md §5, revised — test-level only): an open spec
/// (red body) is its own outcome, never a failure; a spec that passes FAILS demanding the flag's
/// removal; an unmet `requires` still skips. Tallies match testdata/spec.lua.
#[test]
fn spec_outcomes_tally() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("spec.lua"), &mut reporter).expect("run spec.lua");
    assert_eq!(summary.spec, 2, "open specs: assertion, raise");
    assert_eq!(summary.failed, 1, "the honored spec demanding removal");
    assert_eq!(summary.passed, 1, "the ordinary test holds the line");
    assert_eq!(summary.skipped, 1, "requires wins over spec");
    assert!(
        !summary.is_success(),
        "an honored-but-still-flagged spec fails the run"
    );
}

/// Open specs alone do not fail a run — CI stays green while specs are authored ahead of
/// implementation.
#[test]
fn open_specs_do_not_fail_the_run() {
    let mut reporter = NullReporter;
    let summary =
        run_path(&testdata("spec_open_only.lua"), &mut reporter).expect("run spec_open_only.lua");
    assert_eq!(summary.spec, 1);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
    assert!(summary.is_success(), "open specs are not failures");
}

/// `--strict-specs` (driver mode): open specs ARE failures — the implementing agent's loop sees
/// full red.
#[test]
fn strict_specs_turns_open_specs_into_failures() {
    let mut reporter = NullReporter;
    let config = RunConfig::default().with_strict_specs(true);
    let summary = run_path_with(&testdata("spec_open_only.lua"), &mut reporter, &config)
        .expect("run spec_open_only.lua strict");
    assert_eq!(summary.spec, 0, "no spec outcomes in strict mode");
    assert_eq!(summary.failed, 1, "the open spec is a real failure");
    assert!(!summary.is_success());
}

/// `--specs` (the selector): run exactly the leaves carrying a spec flag — unflagged tests are
/// deselected. Green spec'd leaves still fail demanding the flag's removal.
#[test]
fn specs_selector_narrows_to_the_spec_surface() {
    let mut reporter = NullReporter;
    let config = RunConfig::default().with_specs_only(true);
    let summary = run_path_with(&testdata("spec.lua"), &mut reporter, &config)
        .expect("run spec.lua specs-only");
    // Selected: 2 open + 1 honored + 1 requires-skip. Deselected: the ordinary test.
    assert_eq!(summary.spec, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.passed, 0, "unflagged tests are not run");
    assert_eq!(summary.deselected, 1, "the ordinary test is deselected");
}

/// `spec = false` does not exist — an unflagged test is already a full proof. Rejected with the
/// fix, never silently accepted.
#[test]
fn spec_false_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("spec_false.lua"), &mut reporter)
        .expect_err("spec = false must refuse to load");
    let msg = err.to_string();
    assert!(
        msg.contains("spec = false is not a thing"),
        "explains the model: {msg}"
    );
    assert!(msg.contains("full proof"), "names the why: {msg}");
}

/// Spec flags are test-level only: a group-level flag is refused with the fix (flag each open
/// test), never silently inherited.
#[test]
fn group_level_spec_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("spec_group.lua"), &mut reporter)
        .expect_err("a group-level spec flag must refuse to run");
    let msg = err.to_string();
    assert!(
        msg.contains("spec is test-level only"),
        "states the rule: {msg}"
    );
    assert!(
        msg.contains("formats"),
        "names the offending group: {msg}"
    );
}
