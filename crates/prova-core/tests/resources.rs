use std::path::PathBuf;
use std::time::Instant;

use prova_core::{run_path_with, NullReporter, RunConfig};

fn run(file: &str, concurrency: usize) -> (prova_core::Summary, std::time::Duration) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/{file}"));
    let mut reporter = NullReporter;
    let config = RunConfig::new(concurrency);
    let start = Instant::now();
    let summary = run_path_with(&path, &mut reporter, &config).expect("run");
    (summary, start.elapsed())
}

/// Two exclusive holders of the same token must serialize even with concurrency headroom: the
/// writer↔writer conflict forces them one-at-a-time, so ~80ms (two 40ms links), not ~40ms.
#[test]
fn exclusive_resource_serializes_under_concurrency() {
    let (summary, elapsed) = run("resource_exclusive.lua", 8);
    assert_eq!(summary.passed, 2, "both pass");
    assert!(
        elapsed.as_millis() >= 70,
        "expected exclusive holders to serialize (~80ms), took {elapsed:?}"
    );
}

/// Two shared holders of the same token may run concurrently (reader ∥ reader): with concurrency
/// they overlap, so ~40ms rather than the ~80ms an exclusive hold would force.
#[test]
fn shared_resource_runs_concurrently() {
    let (summary, elapsed) = run("resource_shared.lua", 8);
    assert_eq!(summary.passed, 2, "both pass");
    assert!(
        elapsed.as_millis() < 70,
        "expected shared readers to overlap (~40ms), took {elapsed:?}"
    );
}
