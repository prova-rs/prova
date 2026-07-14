use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `requires` gates on capability availability: an unavailable capability SKIPS the unit (not
/// fails), and that skip cascades to dependents. An available capability (or no requirement) runs.
#[test]
fn requires_skips_on_missing_capability() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/requires.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run requires.lua");

    // "needs sh" + "no requirements" + "needs kafka (compiled in)" pass; the two missing-tool gates
    // + the dependent skip. (The kafka assertion assumes the default-featured test build.)
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(
        summary.failed, 0,
        "failed (nothing ever fails on a missing capability)"
    );
    assert_eq!(summary.skipped, 3, "skipped");
}
