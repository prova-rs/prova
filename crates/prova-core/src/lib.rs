//! prova-core — the engine for the `prova` acceptance-test runner.
//!
//! POC vertical slice: inject the `prova` global, collect `prova.test` / `prova.group`, execute
//! each test with an injected `t` context + `t:expect` matchers, and drive a structured event
//! stream to a `Reporter`. Fixtures, flows, dependencies, resources, and timeouts are the next
//! increments; the seams for them live in `model` and `engine` (see the module docs there).

mod engine;
pub mod model;

pub use engine::{discover_path, run_path, run_path_with, RunConfig};
pub use model::{
    ConsoleReporter, Event, JsonReporter, MultiReporter, NullReporter, Outcome, Reporter, Summary,
};
