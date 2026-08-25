//! prova-core — the engine for the `prova` acceptance-test runner.
//!
//! The `prova` global is injected into each file; `engine` collects `prova.test`/`flow`/`group` +
//! `fixture`, builds a leaf dependency-DAG plan, and runs it with a resource-aware async scheduler.
//! `suite` runs many files across a pool of per-worker Lua states (true multi-core). Output is a
//! structured `Event` stream consumed by `Reporter` sinks (`model`).

pub mod barrier;
pub mod baselines;
mod engine;
pub mod help;
pub mod layout;
pub mod model;
mod modules;
mod opts;
mod packages;
pub mod progress;
pub mod suggest;
mod suite;
pub mod lanes;
pub mod locks;
pub mod lease;
pub mod ledger;

/// The reserved-name registry (api-freeze §2): every bundled namespace name, including kernel
/// transports that are designed but not yet shipped — reserving ahead of the implementation is
/// the point, so no plugin claims `socket` the release before prova does. A `[plugins]` entry or
/// plugin-root file bearing one of these is a manifest validation error; assignment to one from
/// test code raises. `prova` and `Scope` are core authoring globals — reserved like the rest and
/// always injected (never optional).
/// The version prova reports — `CARGO_PKG_VERSION`, plus `+dev.<sha>` when this is not a release
/// build. See `build.rs`: the suffix is build metadata precisely so semver comparisons ignore it,
/// which keeps a dev build satisfying the same `[requires] prova` ranges a release would.
pub const VERSION: &str = env!("PROVA_VERSION");

/// The nesting marker every process prova spawns carries: the depth of the run that launched it.
/// Absent (or unparseable) means depth 0 — this process is the top-level run.
///
/// It exists because a handful of prova's behaviors are keyed on ambient environment that answers
/// "what kind of place am I running in?" — `GITHUB_ACTIONS` most of all. That reading is a proxy
/// for "I am the run this job is watching", and the proxy silently breaks the moment prova spawns
/// prova: the environment propagates to the child, but being the job's run does not. A depth the
/// child can read is what tells the two apart.
///
/// A counter, not a boolean, because it propagates through arbitrary levels: each spawn stamps
/// `parent + 1`, so no level has to reason about whether some ancestor already set a flag.
pub const RUN_DEPTH_ENV: &str = "PROVA_RUN_DEPTH";

/// This process's nesting depth: 0 at the top level, +1 for each enclosing prova run.
///
/// Read from the environment rather than carried on `RunConfig`, because the question is asked
/// across a process boundary — the parent stamps, the child asks — and because it must survive the
/// intermediaries a suite legitimately puts in between (`make`, `npm`, a shell wrapper), which
/// forward the environment but know nothing about prova.
pub fn run_depth() -> u32 {
    std::env::var(RUN_DEPTH_ENV).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

/// This process's executable, as a path something can actually **exec**.
///
/// `std::env::current_exe()` is not that on Linux. It reads `/proc/self/exe`, and once the file has
/// been REPLACED the kernel appends a literal `" (deleted)"` to what that link reports — a marker,
/// not a path. This tree replaces its own binary routinely: a run's `[runner]` provisioning rebuilds
/// `target/debug/prova` while the prova doing the running IS that file. Hand the marker to `exec`
/// and you get `not found` (exit 127).
///
/// That is not hypothetical. Three `proofs/spec/stdio/spawnable_test.lua` proofs failed on
/// ubuntu-latest and passed on macOS for four days, because the spawnable shim is
/// `exec <current_exe> relay --to …` and macOS resolves `_NSGetExecutablePath`, which carries no
/// such suffix. The platform difference is the whole bug.
///
/// Strip the marker and prefer the canonical path when something is there — after a rebuild that
/// something is the NEW binary at the same path, which is the right thing to exec: prova's verbs are
/// stable across a rebuild of the same tree, and every caller here wants "a prova", not "the exact
/// inode I booted from".
pub fn current_exe() -> std::io::Result<std::path::PathBuf> {
    let raw = std::env::current_exe()?;
    usable_exe_path(&raw, |p| p.exists())
}

/// The decision `current_exe` makes, with the filesystem passed in so it can be proven.
fn usable_exe_path(
    raw: &std::path::Path,
    exists: impl Fn(&std::path::Path) -> bool,
) -> std::io::Result<std::path::PathBuf> {
    let Some(stripped) = raw.to_str().and_then(|s| s.strip_suffix(" (deleted)")) else {
        return Ok(raw.to_path_buf());
    };
    let candidate = std::path::PathBuf::from(stripped);
    if exists(&candidate) {
        return Ok(candidate);
    }
    // Replaced by nothing. Say so in the vocabulary of the cause, because the symptom downstream is
    // an unrelatable `exec: not found` from inside a generated shim.
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "this prova's executable was replaced while it was running and nothing is at \
             {stripped} now — a rebuild that moved or removed the binary, rather than replacing it \
             in place. Re-run against a binary that still exists."
        ),
    ))
}

pub const RESERVED_NAMESPACES: &[&str] = &[
    "prova", "Scope", "shell", "fs", "net", "http", "docker", "sqlite", "grpc", "graphql",
    "json", "yaml", "toml", "csv", "base64", "hash", "uuid", "url", "socket", "stdio",
    "terminal", "websocket", "path", "str", "junit", "sarif", "measure", "date",
];

/// A bundled namespace that may be INJECTED as an unqualified global via `[globals] inject` — every
/// reserved name except the core authoring globals `prova`/`Scope`, which are always present and are
/// not optional "modules". Also the set a `[globals] inject` entry is validated against: an entry must
/// be one of these, or a declared `[plugins]` name.
pub fn is_injectable_module(name: &str) -> bool {
    name != "prova" && name != "Scope" && RESERVED_NAMESPACES.contains(&name)
}

/// The default unqualified-global set injected when a package declares no `[globals]` section — a
/// CURATED list, deliberately not "everything injectable". The DSL-shaped modules that predate the
/// injection model keep their ambient names (backward-compatible); high-collision utility names
/// (`path`, later `str`) are canonical-only by default — always reachable as `prova.<name>`,
/// ambient only when a package asks via `[globals] inject`. The init archetype writes the list out
/// explicitly, so a real project SEES its globals rather than inheriting an invisible default.
pub const DEFAULT_INJECT: &[&str] = &[
    "shell", "fs", "net", "http", "docker", "sqlite", "grpc", "graphql", "json", "yaml", "toml",
    "csv", "base64", "hash", "uuid", "url", "socket", "stdio", "terminal", "websocket", "junit",
    "sarif", "measure", "date",
];

pub fn default_inject() -> Vec<String> {
    DEFAULT_INJECT.iter().map(|s| s.to_string()).collect()
}

pub use engine::{
    builtin_capability_names, collect_reminders, collect_switch_census, discover_path, discover_path_with, docker_runs_linux_containers, eval_snippet,
    evaluate_reminders, hold_topology,
    inspect_package, is_builtin_capability, list_topologies, load_project_config, obligations_for_suite,
    qualify_leaf_path, resolve_capabilities, run_path, ProofObligation,
    run_path_with, unreferenced_snapshots, up, AttachedRegistry, AttachedTopology,
    Capabilities, CapabilityExplanation, CapabilityFactory, CapabilityRegistration, CommandProbe,
    DeclKind, Endpoint, HeldTopology, InternedHandle, Module, Stream, TopologyPool,
    ListNode, PackageReport, PackageShape, PortMode, RunConfig, Selection, SnapshotRegistry,
    TopologyRegistration, UndeclaredPolicy, VersionQuery,
};
pub use lanes::{Lane, LANES};
pub use layout::{RootedSystemLayout, SystemLayout, XdgSystemLayout};
pub use progress::{Activity, Kind as ActivityKind, NullProgress, Progress};
pub use model::{
    spec_summary_segment, ConsoleReporter, DeputedCase, DeputedRegistry, Direction, Event,
    JUnitReporter, JsonReporter, Measurement, MeasurementRegistry, Report, ReportForm, ReportRegistry, MultiReporter, NullReporter,
    Outcome, ReminderAccount, ReminderListing, ReminderOutcome, ReminderState, Reporter, SpecItem, Summary, TapReporter,
};
pub use suite::{discover_files, discover_suite, discover_suites, is_test_file, run_suite, run_suites, Suite};

/// The one separator convention for every path prova emits — `/`-normalized on Windows,
/// byte-exact elsewhere. Public because the run record is a path-PRODUCING surface too: agents read
/// it and people paste it into shells, so a `\\` in JSON is an escape nobody meant.
pub use crate::modules::emit_path;

#[cfg(test)]
mod exe_path_tests {
    use super::usable_exe_path;
    use std::path::{Path, PathBuf};

    /// The ordinary case: no marker, hand the path straight back. Nothing on the filesystem is even
    /// consulted, so a binary running from a path that has since become unreadable still resolves.
    #[test]
    fn a_plain_path_is_returned_untouched() {
        let raw = PathBuf::from("/usr/local/bin/prova");
        let got = usable_exe_path(&raw, |_| panic!("must not stat a path with no marker")).unwrap();
        assert_eq!(got, raw);
    }

    /// The Linux case this exists for: `/proc/self/exe` reports the marker after the binary is
    /// REPLACED, and the replacement is sitting at the same path. Exec that.
    #[test]
    fn a_replaced_binary_resolves_to_the_path_without_the_marker() {
        let raw = PathBuf::from("/w/target/debug/prova (deleted)");
        let got = usable_exe_path(&raw, |p| p == Path::new("/w/target/debug/prova")).unwrap();
        assert_eq!(got, PathBuf::from("/w/target/debug/prova"));
        assert!(
            !got.to_string_lossy().contains("(deleted)"),
            "the marker must never reach an exec — that is the `not found` (127) this fixes"
        );
    }

    /// Replaced by NOTHING — a rebuild that moved or removed the binary. Refuse, in the vocabulary
    /// of the cause: the downstream symptom is an unrelatable `exec: not found` from inside a
    /// generated shim, which is what made this cost four days of green-on-macOS/red-on-Linux.
    #[test]
    fn a_deleted_binary_with_no_replacement_is_an_error_that_explains_itself() {
        let raw = PathBuf::from("/w/target/debug/prova (deleted)");
        let e = usable_exe_path(&raw, |_| false).expect_err("nothing to exec");
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        let msg = e.to_string();
        assert!(msg.contains("/w/target/debug/prova"), "{msg}");
        assert!(msg.contains("replaced while it was running"), "{msg}");
    }

    /// A path that merely CONTAINS the words is not a marker — only the suffix is. A directory
    /// honestly named "… (deleted)" must not be silently rewritten out from under the caller.
    #[test]
    fn the_marker_is_a_suffix_not_a_substring() {
        let raw = PathBuf::from("/w/archive (deleted)/prova");
        let got = usable_exe_path(&raw, |_| panic!("must not stat: this path has no trailing marker"))
            .unwrap();
        assert_eq!(got, raw);
    }

    /// The real call answers on this host, whatever it is — the integration leg the pure cases
    /// above cannot cover.
    #[test]
    fn current_exe_resolves_to_something_that_exists_here() {
        let p = super::current_exe().expect("this test binary has a path");
        assert!(p.exists(), "resolved {p:?}, which is not there");
    }
}
