use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `http.mock` — the `mock` facet: stub, drive, record, assert.
///
/// No docker guard: the whole point of an in-process mock is that it needs no daemon, so this runs
/// concurrently with everything else and stays fast.
///
/// The load-bearing case in here is "a Lua handler answers while the test is suspended". It is the
/// one that fails if the engine ever stops driving local tasks alongside test coroutines — e.g. if a
/// `block_on` is added that doesn't go through `block_on_local`, `spawn_local` panics outright. That
/// is a wiring regression a unit test would never catch, which is why it is proved end-to-end here.
#[test]
fn http_mock_stubs_records_and_runs_lua_handlers() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("http_mock.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run http_mock.lua");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 18, "passed");
}
