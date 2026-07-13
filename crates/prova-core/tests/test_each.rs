use std::path::PathBuf;

use prova_core::{discover_path, run_path, NullReporter};

fn testdata(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(file)
}

/// `test_each` generates one passing test per case (top-level and inside a group), the case reaches
/// the body both as the 2nd argument and as `t.case`, and an ordinary test is unaffected.
/// 3 + 2 + 2 parametrized + 1 plain = 8 tests, all green.
#[test]
fn test_each_generates_one_test_per_case() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("test_each.lua"), &mut reporter).expect("run test_each.lua");
    assert_eq!(summary.passed, 8, "one test per case + the plain test");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.skipped, 0, "skipped");
}

/// The `{placeholder}` name template is filled from each case, so discovery reports distinct,
/// substituted test names.
#[test]
fn test_each_names_fill_placeholders() {
    let names = discover_path(&testdata("test_each.lua")).expect("discover test_each.lua");
    assert!(
        names.iter().any(|n| n.ends_with("squares 3")),
        "expected a substituted name 'squares 3', got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("doubles 6")),
        "expected a substituted group name 'doubles 6', got: {names:?}"
    );
    // No unsubstituted placeholder should survive.
    assert!(
        !names.iter().any(|n| n.contains('{')),
        "no placeholder should remain: {names:?}"
    );
}
