use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `is_fully_rendered` passes on a clean tree (GitHub `${{ }}` excluded) and fails on a leftover
/// marker in contents, in a block/comment tag, or in a path segment — all four tests green (the
/// negative cases use `:never()`).
#[test]
fn is_fully_rendered_detects_leftover_markers() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("fully_rendered.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run fully_rendered.lua");
    assert_eq!(summary.passed, 4, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
