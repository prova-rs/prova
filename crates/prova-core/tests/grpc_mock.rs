use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// `grpc.mock` — the `mock` facet on the grpc namespace.
///
/// No docker guard and no network: the mock is in-process and the schema is compiled at runtime by
/// protox, so this is hermetic and fast. (`tests/grpc.rs` covers the client against a real server.)
///
/// The bar every case here shares is that the driving client is `grpc.client`, unmodified — it
/// learns the mock's schema over reflection exactly as it would a real server's. Two regressions
/// this catches that nothing else would: reflection silently not being served (the client cannot
/// even connect), and the `!Send` path breaking — a Lua handler answering an RPC requires
/// `UnaryService::Future` to stay unbounded and hyper's http2 executor to stay `LocalExec`, and
/// swapping in `TokioExecutor` fails to compile rather than failing here.
#[test]
fn grpc_mock_serves_reflection_stubs_statuses_and_lua_handlers() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("grpc_mock.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run grpc_mock.lua");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 12, "passed");
}
