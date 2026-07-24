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
mod suite;

pub use engine::{
    discover_path, discover_path_with, docker_runs_linux_containers, eval_snippet, hold_topology,
    inspect_plugin, is_builtin_capability, list_topologies, load_project_config, run_path,
    run_path_with, unreferenced_snapshots, up, watch, Capabilities, Endpoint, HeldTopology, Module,
    PluginReport, PluginShape, PortMode, RunConfig, Selection, SnapshotRegistry,
    TopologyRegistration,
};
pub use layout::{RootedSystemLayout, SystemLayout, XdgSystemLayout};
pub use model::{
    spec_summary_segment, ConsoleReporter, Event, JUnitReporter, JsonReporter, MultiReporter,
    NullReporter, Outcome, Reporter, SpecOpt, Summary, TapReporter,
};
pub use suite::{discover_files, discover_suite, discover_suites, run_suite, run_suites, Suite};
