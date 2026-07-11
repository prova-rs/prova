use std::path::PathBuf;
use std::time::Instant;

use prova_core::{discover_files, run_suite, NullReporter, RunConfig};

fn testdata(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(sub)
}

/// Discovery finds both `*_test.lua` files in a directory, and the suite runner aggregates every
/// file's outcomes into one summary.
#[test]
fn runs_a_multi_file_suite_and_aggregates() {
    let dir = testdata("suite");
    let files = discover_files(&dir).expect("discover");
    assert_eq!(files.len(), 2, "found both test files");

    let mut reporter = NullReporter;
    let config = RunConfig { concurrency: 4 };
    let summary = run_suite(&files, &mut reporter, &config).expect("run suite");

    // alpha: 2 pass + 1 fail; beta: 1 pass + 1 skip.
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 1, "failed");
    assert_eq!(summary.skipped, 1, "skipped");
}

/// True multi-core: two CPU-bound files (busy loops that never await) can only overlap on separate
/// worker threads with separate Lua states. Running the same suite at `--jobs 2` must be clearly
/// faster than at `--jobs 1` — a ratio test, robust to absolute machine speed.
#[test]
fn cpu_bound_files_parallelize_across_workers() {
    let files = discover_files(&testdata("suite_cpu")).expect("discover");
    assert_eq!(files.len(), 2, "two cpu files");

    let time = |jobs: usize| {
        let mut reporter = NullReporter;
        let config = RunConfig { concurrency: jobs };
        let start = Instant::now();
        let summary = run_suite(&files, &mut reporter, &config).expect("run");
        assert_eq!(summary.passed, 2, "both cpu tests pass at jobs={jobs}");
        start.elapsed()
    };

    let serial = time(1);
    let parallel = time(2);
    assert!(
        parallel.as_secs_f64() < serial.as_secs_f64() * 0.75,
        "expected 2 workers to beat 1 on CPU-bound files: serial={serial:?} parallel={parallel:?}"
    );
}
