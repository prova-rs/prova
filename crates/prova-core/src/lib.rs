//! prova-core — the engine for the `prova` acceptance-test runner.
//!
//! The `prova` global is injected into each file; `engine` collects `prova.test`/`flow`/`group` +
//! `fixture`, builds a leaf dependency-DAG plan, and runs it with a resource-aware async scheduler.
//! `suite` runs many files across a pool of per-worker Lua states (true multi-core). Output is a
//! structured `Event` stream consumed by `Reporter` sinks (`model`).

pub mod baselines;
mod engine;
pub mod help;
pub mod layout;
pub mod model;
mod modules;
mod packages;
pub mod progress;
mod suite;
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

pub const RESERVED_NAMESPACES: &[&str] = &[
    "prova", "Scope", "shell", "fs", "net", "http", "docker", "sqlite", "grpc", "graphql",
    "json", "yaml", "toml", "csv", "base64", "hash", "uuid", "url", "socket", "terminal",
    "websocket", "path", "str", "junit", "sarif", "measure", "date",
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
    "csv", "base64", "hash", "uuid", "url", "socket", "terminal", "websocket", "junit", "sarif",
    "measure", "date",
];

pub fn default_inject() -> Vec<String> {
    DEFAULT_INJECT.iter().map(|s| s.to_string()).collect()
}

pub use engine::{
    collect_reminders, discover_path, discover_path_with, docker_runs_linux_containers, eval_snippet,
    evaluate_reminders, hold_topology,
    inspect_package, is_builtin_capability, list_topologies, load_project_config, obligations_for_suite,
    qualify_leaf_path, run_path, ProofObligation,
    run_path_with, unreferenced_snapshots, up, watch, AttachedRegistry, AttachedTopology,
    Capabilities, Endpoint, HeldTopology, Module,
    PackageReport, PackageShape, PortMode, RunConfig, Selection, SnapshotRegistry,
    TopologyRegistration,
};
pub use layout::{RootedSystemLayout, SystemLayout, XdgSystemLayout};
pub use progress::{Activity, Kind as ActivityKind, NullProgress, Progress};
pub use model::{
    spec_summary_segment, ConsoleReporter, DeputedCase, DeputedRegistry, Direction, Event,
    JUnitReporter, JsonReporter, Measurement, MeasurementRegistry, MultiReporter, NullReporter,
    DatedObligation, Outcome, ReminderAccount, ReminderListing, ReminderOutcome, ReminderState, Reporter, Summary, TapReporter,
};
pub use suite::{discover_files, discover_suite, discover_suites, is_test_file, run_suite, run_suites, Suite};
