use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `yaml.parse` (single doc → table) and `yaml.parse_all` (multi-document `---` stream), plus a
/// raise on invalid YAML — all three tests green.
#[test]
fn yaml_module_parses_single_and_multi_doc() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("yaml.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run yaml.lua");
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
