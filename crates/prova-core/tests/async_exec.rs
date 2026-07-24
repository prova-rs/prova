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

    // Two 400ms sleepers pass; the over-budget test is cancelled → failed.
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 1, "failed (timeout)");
    assert_eq!(summary.skipped, 0, "skipped");

    // Concurrency proof by construction, immune to slow runners: ANY sequential schedule must
    // sleep 400+400+50 ≥ 850ms of pure wall-clock before overhead, while the concurrent run
    // sleeps ~400ms. A sub-800ms finish is impossible without overlap, however slow the
    // machine adds overhead on top.
    assert!(
        elapsed < Duration::from_millis(800),
        "expected concurrent execution, took {elapsed:?}"
    );
}
