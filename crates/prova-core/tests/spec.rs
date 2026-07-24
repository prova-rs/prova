use std::path::PathBuf;

use prova_core::{run_path, run_path_with, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// The `spec` lifecycle (docs/plans/api-freeze.md §5): an open spec (red body) is its own outcome,
/// never a failure; a spec that passes FAILS demanding graduation; `spec = false` graduates a leaf
/// back to ordinary; an unmet `requires` still skips. Tallies match testdata/spec.lua.
#[test]
fn spec_outcomes_tally() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("spec.lua"), &mut reporter).expect("run spec.lua");
    assert_eq!(
        summary.spec, 3,
        "open specs: assertion, raise, group-inherited"
    );
    assert_eq!(summary.failed, 1, "the honored spec demanding graduation");
    assert_eq!(summary.passed, 1, "the graduated test holds the line");
    assert_eq!(summary.skipped, 1, "requires wins over spec");
    assert_eq!(summary.graduated, 1, "graduation markers are counted");
    assert!(
        !summary.is_success(),
        "an honored-but-unflagged spec fails the run"
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

/// `--specs` (the selector): run exactly the leaves carrying an effective spec flag — graduated
/// leaves and ordinary tests are deselected. Green spec'd leaves still fail demanding graduation.
#[test]
fn specs_selector_narrows_to_the_spec_surface() {
    let mut reporter = NullReporter;
    let config = RunConfig::default().with_specs_only(true);
    let summary = run_path_with(&testdata("spec.lua"), &mut reporter, &config)
        .expect("run spec.lua specs-only");
    // Selected: 3 open + 1 honored + 1 requires-skip. Deselected: the graduated leaf.
    assert_eq!(summary.spec, 3);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.passed, 0, "graduated/ordinary leaves are not run");
    assert_eq!(summary.deselected, 1, "the graduated leaf is deselected");
}

/// A `spec = false` with no enclosing spec flag is a validation error — stale graduation markers
/// cannot linger.
#[test]
fn orphan_graduation_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("spec_orphan.lua"), &mut reporter)
        .expect_err("orphan graduation must refuse to run");
    let msg = err.to_string();
    assert!(
        msg.contains("spec = false") && msg.contains("no enclosing spec flag"),
        "names the marker and explains: {msg}"
    );
    assert!(
        msg.contains("orphan graduation"),
        "names the offending node: {msg}"
    );
}

/// When every leaf under a spec flag has graduated, the flag is complete — the run errors until
/// the flag and its graduation markers are removed.
#[test]
fn completed_spec_flag_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("spec_complete.lua"), &mut reporter)
        .expect_err("a fully-graduated spec flag must refuse to run");
    let msg = err.to_string();
    assert!(
        msg.contains("spec complete") && msg.contains("done"),
        "announces completion and names the flag: {msg}"
    );
    assert!(
        msg.contains("remove the flag"),
        "tells the author the cleanup step: {msg}"
    );
}
