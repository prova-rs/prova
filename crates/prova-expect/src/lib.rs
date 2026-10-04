//! prova-expect — prova's assertion core: the `t:expect` matchers, `:eventually`, snapshots, the
//! structural compare/display helpers, and the per-test record ([`TestRun`]) they write.
//!
//! It is its own crate so every Lua host links ONE implementation: the prova engine, and a host
//! that embeds prova's proof API in a VM of its own (Substrate's vault proof host). Everything here
//! is `Send`: the per-test record is shared through [`RunHandle`] (`Arc<Mutex<…>>`), never an
//! `Rc<RefCell<…>>`, so a host built with mlua's `send` feature links it unchanged. In prova's own
//! single-threaded engine the lock is uncontended. No lock is ever held across an `.await`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[allow(unused_imports)]
use mlua::{Lua, Table, UserData, UserDataMethods, Value};

mod matchers;
pub use matchers::*;

/// One test's record: what its assertions wrote.
#[derive(Default)]
pub struct TestRun {
    pub assertions: usize,
    pub failure: Option<String>,
    pub skip: Option<String>,
    /// Inside `t:expect_all(...)`, a failed assertion is collected here instead of aborting, so the
    /// block reports *every* failure. `soft` is the active flag; `soft_failures` accumulates.
    pub soft: bool,
    pub soft_failures: Vec<String>,
    /// Snapshot context for `matches_snapshot` (where `.snap` files live, the key base, update mode,
    /// and a per-test counter for auto-named snapshots). `None` when the test has no source file path.
    pub snapshot: Option<SnapshotCtx>,
}

/// The shared handle to a test's record: every matcher of the test writes through it.
pub type RunHandle = Arc<Mutex<TestRun>>;

/// A fresh, empty record.
pub fn new_run() -> RunHandle {
    Arc::new(Mutex::new(TestRun::default()))
}

/// Lock a shared value. A poisoned lock (a panic while held) still yields its data: the record is
/// plain counters and strings, never left half-invariant, and a test run must report, not abort.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Per-test snapshot state: everything `matches_snapshot` needs to locate and key a `.snap` file.
pub struct SnapshotCtx {
    /// `<test-file-dir>/snapshots`.
    pub dir: PathBuf,
    /// The test-file stem — the `.snap` filename prefix (`<stem>__<key>.snap`).
    pub stem: String,
    /// A slug of the test's node path — the base for auto-named snapshots (`<slug>-<n>`).
    pub key_base: String,
    /// `--update-snapshots`: write instead of compare.
    pub update: bool,
    /// Increments per *unnamed* `matches_snapshot` in this test, so several are distinct.
    pub counter: usize,
    /// Shared registry to record each referenced `.snap` into (for unreferenced reconciliation).
    pub registry: Option<SnapshotRegistry>,
}

/// A thread-safe set of every `.snap` file referenced during a run — shared across worker Lua states
/// so the CLI can find untouched (orphaned) snapshots afterward.
pub type SnapshotRegistry = Arc<Mutex<std::collections::HashSet<PathBuf>>>;

/// A duration in prova's grammar: a number with an optional unit — "250ms", "30s", "5m", or a bare
/// number read as seconds.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(x) = s.strip_suffix("ms") {
        return x
            .trim()
            .parse::<f64>()
            .ok()
            .map(Duration::from_secs_f64)
            .map(|d| d / 1000);
    }
    if let Some(x) = s.strip_suffix('s') {
        return x.trim().parse::<f64>().ok().map(Duration::from_secs_f64);
    }
    if let Some(x) = s.strip_suffix('m') {
        return x
            .trim()
            .parse::<f64>()
            .ok()
            .map(|m| Duration::from_secs_f64(m * 60.0));
    }
    s.parse::<f64>().ok().map(Duration::from_secs_f64)
}
