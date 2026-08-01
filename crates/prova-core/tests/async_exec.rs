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

    // Two 1500ms sleepers pass; the over-budget test is cancelled → failed.
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 1, "failed (timeout)");
    assert_eq!(summary.skipped, 0, "skipped");

    // Concurrency proof by construction: ANY sequential schedule must sleep 1500+1500+50
    // ≥ 3050ms of pure wall-clock before overhead, while the concurrent run sleeps ~1500ms.
    // A sub-2900ms finish is impossible without overlap. The sleeps are this long (not 400ms)
    // so the margin dominates runner overhead — the Windows CI runner was measured adding
    // ~900ms of startup/scan noise, which a tighter construction misread as sequentialism.
    assert!(
        elapsed < Duration::from_millis(2900),
        "expected concurrent execution, took {elapsed:?}"
    );
}
