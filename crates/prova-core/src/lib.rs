//! prova-core — the engine for the `prova` acceptance-test runner.
//!
//! The `prova` global is injected into each file; `engine` collects `prova.test`/`flow`/`group` +
//! `fixture`, builds a leaf dependency-DAG plan, and runs it with a resource-aware async scheduler.
//! `suite` runs many files across a pool of per-worker Lua states (true multi-core). Output is a
//! structured `Event` stream consumed by `Reporter` sinks (`model`).

mod engine;
pub mod help;
pub mod layout;
pub mod model;
mod modules;
mod plugins;
pub mod progress;
mod suite;

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

pub const RESERVED_NAMESPACES: &[&str] = &[
    "prova", "Scope", "shell", "fs", "net", "http", "docker", "sqlite", "grpc", "graphql",
    "json", "yaml", "toml", "csv", "base64", "hash", "uuid", "url", "socket", "terminal",
    "websocket", "path",
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
    "csv", "base64", "hash", "uuid", "url", "socket", "terminal", "websocket",
];

pub fn default_inject() -> Vec<String> {
    DEFAULT_INJECT.iter().map(|s| s.to_string()).collect()
}

pub use engine::{
    discover_path, discover_path_with, docker_runs_linux_containers, eval_snippet, hold_topology,
    inspect_plugin, is_builtin_capability, list_topologies, load_project_config, run_path,
    run_path_with, unreferenced_snapshots, up, watch, Capabilities, Endpoint, HeldTopology, Module,
    PluginReport, PluginShape, PortMode, RunConfig, Selection, SnapshotRegistry,
    TopologyRegistration,
};
pub use layout::{RootedSystemLayout, SystemLayout, XdgSystemLayout};
pub use progress::{Activity, Kind as ActivityKind, NullProgress, Progress};
pub use model::{
    spec_summary_segment, ConsoleReporter, Event, JUnitReporter, JsonReporter, MultiReporter,
    NullReporter, Outcome, Reporter, Summary, TapReporter,
};
pub use suite::{discover_files, discover_suite, discover_suites, is_test_file, run_suite, run_suites, Suite};
