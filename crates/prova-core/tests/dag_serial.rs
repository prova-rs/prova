use std::path::PathBuf;
use std::time::Instant;

use prova_core::{run_path_with, NullReporter, RunConfig};

/// A dependency edge must **order and gate**, not just annotate: even with concurrency headroom, a
/// chain a → b → c cannot overlap. Each link sleeps 40ms; if the scheduler honored deps the run is
/// ~120ms, and if it ignored them (ran all three at once) it would be ~40ms. We assert it took
/// clearly longer than a single link, proving the chain serialized despite `concurrency = 8`.
#[test]
fn dependency_edges_serialize_under_concurrency() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/dag_serial.lua");
    let mut reporter = NullReporter;
    let config = RunConfig { concurrency: 8 };

    let start = Instant::now();
    let summary = run_path_with(&path, &mut reporter, &config).expect("run dag_serial.lua");
    let elapsed = start.elapsed();

    assert_eq!(summary.passed, 3, "all three links pass");
    assert_eq!(summary.failed, 0, "no failures");
    // Serialized ≈ 120ms. Allow slack but require well above one link (40ms) to prove no overlap.
    assert!(
        elapsed.as_millis() >= 100,
        "expected the chain to serialize (~120ms), took {elapsed:?} — deps were not honored"
    );
}
