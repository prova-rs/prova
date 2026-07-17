use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `http.mock` passthrough / record / replay — the observe dial.
///
/// Hermetic: the "real service" each proxy forwards to is another `http.mock`, so this needs no
/// docker and no network. That is not a shortcut — a mock standing in for the real service *for its
/// own proxy* is the same claim the facet makes to a system under test.
///
/// The case that matters most is "record, then replay with the dependency gone": it is the answer to
/// the drift objection that sinks most mocking, and it is only an answer if the *same* assertions
/// pass on both sides of it.
#[test]
fn mock_passthrough_records_and_replays() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("mock_proxy.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run mock_proxy.lua");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 12, "passed");
}
