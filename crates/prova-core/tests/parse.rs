use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `prova.parse.*` — lines / rows / table (column-by-header) / json (null→nil). No docker needed.
#[test]
fn parse_toolkit() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/parse.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run parse.lua");
    assert_eq!(summary.passed, 4, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
