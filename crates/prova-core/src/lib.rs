//! prova-core — the engine for the `prova` acceptance-test runner.
//!
//! The `prova` global is injected into each file; `engine` collects `prova.test`/`flow`/`group` +
//! `fixture`, builds a leaf dependency-DAG plan, and runs it with a resource-aware async scheduler.
//! `suite` runs many files across a pool of per-worker Lua states (true multi-core). Output is a
//! structured `Event` stream consumed by `Reporter` sinks (`model`).

mod engine;
pub mod layout;
pub mod model;
mod modules;
mod plugins;
mod suite;

pub use engine::{
    Selection,
    discover_path, discover_path_with, inspect_plugin, run_path, run_path_with, up, watch, Endpoint,
    Module, PluginReport, PluginShape, PortMode, RunConfig,
};
pub use layout::{RootedSystemLayout, SystemLayout, XdgSystemLayout};
pub use model::{
    ConsoleReporter, Event, JsonReporter, MultiReporter, NullReporter, Outcome, Reporter, Summary,
};
pub use suite::{discover_files, discover_suites, run_suite, run_suites, Suite};
