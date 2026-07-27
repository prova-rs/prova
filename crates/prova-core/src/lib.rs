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
/// test code raises. `prova` and `Scope` are core authoring globals — reserved like the rest but
/// never excludable from injection.
pub const RESERVED_NAMESPACES: &[&str] = &[
    "prova", "Scope", "shell", "fs", "net", "http", "docker", "sqlite", "grpc", "graphql",
    "json", "yaml", "toml", "csv", "base64", "hash", "uuid", "url", "socket", "terminal",
    "websocket",
];

/// The names a manifest may exclude from global injection: the reserved set minus the core
/// authoring globals a test cannot function without.
pub fn excludable_namespace(name: &str) -> bool {
    name != "prova" && name != "Scope" && RESERVED_NAMESPACES.contains(&name)
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
