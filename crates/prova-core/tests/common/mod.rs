//! Shared helpers for the integration tests.
//!
//! (In `tests/common/` rather than `tests/` so cargo treats it as a module to include, not as a test
//! binary of its own.)

use std::fs::File;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use prova_core::{run_path, Event, Outcome, Reporter, Summary};

/// A reporter that keeps the failure messages, so a failing proof can say what went wrong.
///
/// `NullReporter` discards them, which made every docker flake look identical from the outside: the
/// harness could only report `failed == 1`, and the actual message — the one naming the container,
/// the timeout, the exit code — was thrown away. Diagnosing those failures meant re-running the Lua
/// by hand and hoping to reproduce. Keeping the messages costs nothing and is the difference between
/// "docker is flaky" and a specific, fixable cause.
// This module is shared verbatim with prova-cli's tests (via `#[path]`), and not every consumer
// needs every helper — an unused one there is expected, not a defect.
#[allow(dead_code)]
#[derive(Default)]
pub struct FailureCapture {
    pub failures: Vec<String>,
}

impl Reporter for FailureCapture {
    fn event(&mut self, event: &Event) {
        if let Event::NodeFinished {
            path,
            outcome: Outcome::Failed,
            message,
            ..
        } = event
        {
            self.failures
                .push(format!("{path}: {}", message.unwrap_or("(no message)")));
        }
    }
}

/// Run a proof file, returning its summary — and panicking with the captured failure messages if
/// anything failed, so the harness assertion never has to report a bare count.
#[allow(dead_code)]
pub fn run_proof(path: &Path) -> Summary {
    let mut reporter = FailureCapture::default();
    let summary =
        run_path(path, &mut reporter).unwrap_or_else(|e| panic!("run {}: {e}", path.display()));
    assert!(
        reporter.failures.is_empty(),
        "{} reported {} failure(s):\n  - {}",
        path.display(),
        reporter.failures.len(),
        reporter.failures.join("\n  - ")
    );
    summary
}

/// Held for as long as a test needs exclusive use of the Docker daemon; releases on drop.
pub struct DockerGuard(File);

impl Drop for DockerGuard {
    fn drop(&mut self) {
        // Closing the file releases the lock anyway; unlock explicitly so the intent is legible and
        // the next binary is unblocked at a defined point rather than at an implicit close.
        let _ = FileExt::unlock(&self.0);
    }
}

/// Serialize this test against every other docker-heavy test binary.
///
/// **Why this exists.** `cargo test` runs each test *binary* as its own **process**, and it runs
/// those processes **in parallel**. `--test-threads=1` bounds only the threads *within* a binary, so
/// it does not help: all eight docker binaries still hit one daemon simultaneously. Under that load
/// container starts crawl, and a suite that passes solo in ~1.5s fails on a 60s timeout — a
/// recurring flake, and an expensive one, because it looks like a real regression every time. (Tell
/// them apart by speed: the flake is a *timeout*; a logic failure fails fast.)
///
/// Since the contention is across processes, the mutex has to be too — hence a file lock. The lock
/// file lives under `CARGO_TARGET_TMPDIR`, which is per-target-directory, so concurrent runs of
/// *different* checkouts (or CI jobs) don't serialize against each other pointlessly.
///
/// Call it first in any test that starts containers:
///
/// ```ignore
/// mod common;
///
/// #[test]
/// fn my_docker_test() {
///     let _docker = common::docker_guard();
///     // …
/// }
/// ```
///
/// The cost is wall-clock: the docker binaries now run one at a time (tens of seconds total). That
/// is the trade — a slower suite that means something, over a faster one that cries wolf.
pub fn docker_guard() -> DockerGuard {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("prova-docker-tests.lock");
    let file = File::create(&path)
        .unwrap_or_else(|e| panic!("create docker test lock at {}: {e}", path.display()));
    // Blocks until whichever binary holds it finishes. No timeout: waiting is the entire point, and
    // a test binary that hangs forever is a better signal than one that silently runs concurrently.
    FileExt::lock_exclusive(&file)
        .unwrap_or_else(|e| panic!("lock docker test lock at {}: {e}", path.display()));
    DockerGuard(file)
}
