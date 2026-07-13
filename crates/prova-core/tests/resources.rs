use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use prova_core::{run_path_with, NullReporter, RunConfig};

/// Run a testdata file at `concurrency`, returning its summary and the ordered event log the file
/// builds by calling the injected `record(event)` global.
///
/// What's under test is *overlap* — whether two holders of a resource may be inside their bodies at
/// the same time — not speed. Inferring that from wall-clock duration needs a threshold calibrated
/// to the machine, and a loaded runner blows straight through it: a macOS CI runner took 123ms for
/// ~40ms of work, which is longer than even the serialized case, so the thresholds both failed the
/// shared test and made the exclusive one pass vacuously. Event order answers the same question
/// exactly, at any speed.
fn run(file: &str, concurrency: usize) -> (prova_core::Summary, Vec<String>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/{file}"));

    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&events);
    let config = RunConfig::new(concurrency).with_module(move |lua| {
        let sink = Arc::clone(&sink);
        let record = lua.create_function(move |_, event: String| {
            sink.lock().expect("event log").push(event);
            Ok(())
        })?;
        lua.globals().set("record", record)?;
        Ok(())
    });

    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &config).expect("run");
    let events = events.lock().expect("event log").clone();
    (summary, events)
}

/// The positions of `enter <name>` and `exit <name>` in the log: the span over which that test held
/// the resource.
fn interval(events: &[String], name: &str) -> (usize, usize) {
    let find = |event: String| {
        events
            .iter()
            .position(|e| *e == event)
            .unwrap_or_else(|| panic!("{event:?} missing from event log {events:?}"))
    };
    (find(format!("enter {name}")), find(format!("exit {name}")))
}

/// Two exclusive holders of the same token must serialize even with concurrency headroom: the
/// writer↔writer conflict forces them one-at-a-time, so their spans are disjoint.
#[test]
fn exclusive_resource_serializes_under_concurrency() {
    let (summary, events) = run("resource_exclusive.lua", 8);
    assert_eq!(summary.passed, 2, "both pass");

    let (enter_a, exit_a) = interval(&events, "a");
    let (enter_b, exit_b) = interval(&events, "b");
    assert!(
        exit_a < enter_b || exit_b < enter_a,
        "expected exclusive holders to serialize (disjoint spans), got {events:?}"
    );
}

/// Two shared holders of the same token may run concurrently (reader ∥ reader): one enters while the
/// other is still inside, so their spans interleave.
#[test]
fn shared_resource_runs_concurrently() {
    let (summary, events) = run("resource_shared.lua", 8);
    assert_eq!(summary.passed, 2, "both pass");

    let (enter_a, exit_a) = interval(&events, "a");
    let (enter_b, exit_b) = interval(&events, "b");
    assert!(
        enter_a < exit_b && enter_b < exit_a,
        "expected shared readers to overlap (interleaved spans), got {events:?}"
    );
}
