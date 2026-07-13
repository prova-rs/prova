use std::path::PathBuf;

use prova_core::{discover_path, run_path, NullReporter};

fn testdata(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(file)
}

/// Every test under (possibly nested) `describe` labels runs and passes: 2 in `math`/`math › nested`
/// + 1 test_each + 1 top-level + 1 in the group's `describe` = 5.
#[test]
fn describe_tests_all_run() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("describe.lua"), &mut reporter).expect("run describe.lua");
    assert_eq!(summary.passed, 5, "all describe-nested tests run");
    assert_eq!(summary.failed, 0, "failed");
}

/// `describe` labels appear in the reported path, nest, and pop back to the root afterward.
#[test]
fn describe_nests_labels_in_paths() {
    let names = discover_path(&testdata("describe.lua")).expect("discover describe.lua");
    assert!(
        names.iter().any(|n| n.contains("math › adds")),
        "expected 'math › adds', got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("math › nested › multiplies")),
        "expected nested label path, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("math › nested › squares 2")),
        "expected test_each nested under describe, got: {names:?}"
    );
    // The post-describe test is back at the root (only the file-stem ancestor, no 'math').
    let top = names
        .iter()
        .find(|n| n.ends_with("top level again"))
        .expect("top-level test present");
    assert!(
        !top.contains("math"),
        "the post-describe test must pop back to root, got: {top:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("outer › subsection › runs")),
        "expected GroupBuilder:describe nesting, got: {names:?}"
    );
}
