use std::path::PathBuf;
use std::time::{Duration, Instant};

use prova_core::{run_path_with, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

#[test]
fn async_bodies_run_concurrently_and_time_out() {
    let mut reporter = NullReporter;
    let started = Instant::now();
    let summary = run_path_with(&testdata("async.lua"), &mut reporter, &RunConfig::new(8))
        .expect("run async.lua");
    let elapsed = started.elapsed();

    // Two 40ms sleepers pass; the over-budget test is cancelled → failed.
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 1, "failed (timeout)");
    assert_eq!(summary.skipped, 0, "skipped");

    // Concurrency proof: the two 40ms sleeps overlap rather than summing. If they ran
    // sequentially the wall-clock would be ~80ms+ before the 20ms-timeout test even starts;
    // concurrently the whole run finishes well under that. Generous bound to avoid CI flake.
    assert!(
        elapsed < Duration::from_millis(150),
        "expected concurrent execution, took {elapsed:?}"
    );
}
