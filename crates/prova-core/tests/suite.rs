use std::path::PathBuf;

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
    let config = RunConfig::new(4);
    let summary = run_suite(&files, &mut reporter, &config).expect("run suite");

    // alpha: 2 pass + 1 fail; beta: 1 pass + 1 skip.
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 1, "failed");
    assert_eq!(summary.skipped, 1, "skipped");
}

/// True multi-core: two CPU-bound files can only overlap on separate worker threads with separate
/// Lua states. Asserted as a RENDEZVOUS — each file publishes a marker, then spins until it sees
/// the other's — so the thing observed is the overlap itself, satisfied or not.
///
/// This replaces a stopwatch: time the suite at jobs=1 and jobs=2, demand the second be 25% faster.
/// That ratio measures the runner's spare capacity as much as prova's scheduling, and on a
/// contended 2-core Windows CI box two workers beat one by only 12% — a red leg with parallelism
/// working exactly as designed. Under branch protection a test that fails on machine load is not a
/// slow signal, it is a blocked merge, so the fix is to stop measuring a proxy for the property and
/// measure the property.
#[test]
fn cpu_bound_files_parallelize_across_workers() {
    let files = discover_files(&testdata("suite_cpu")).expect("discover");
    assert_eq!(files.len(), 2, "two cpu files");

    // How the pair finds each other. Set once, before any run: the other test in this binary reads
    // no environment, and both runs below share the directory (emptied between them).
    let dir = std::env::temp_dir().join(format!("prova-rendezvous-{}", std::process::id()));
    std::env::set_var("PROVA_RENDEZVOUS_DIR", &dir);

    let run = |jobs: usize| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("rendezvous dir");
        let mut reporter = NullReporter;
        run_suite(&files, &mut reporter, &RunConfig::new(jobs)).expect("run")
    };

    // Two workers: both files are in flight together, so both halves see their partner.
    let parallel = run(2);
    assert_eq!(
        (parallel.passed, parallel.failed),
        (2, 0),
        "two workers must overlap the files: {parallel:?}"
    );

    // One worker: the first file waits for a partner that cannot start until it returns, so it
    // times out. The second then finds the first's marker already on disk and passes. Exactly one
    // failure is the negative control — it shows what passed above was the overlap and not merely
    // the presence of two files on disk.
    let serial = run(1);
    assert_eq!(
        (serial.passed, serial.failed),
        (1, 1),
        "one worker cannot overlap them: {serial:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
