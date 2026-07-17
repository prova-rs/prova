//! Shared helpers for the integration tests.
//!
//! (In `tests/common/` rather than `tests/` so cargo treats it as a module to include, not as a test
//! binary of its own.)

use std::fs::File;
use std::path::PathBuf;

use fs2::FileExt;

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
