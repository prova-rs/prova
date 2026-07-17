use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// A mock reply-handler that raises fails the scope that owns the mock.
///
/// **3 failed is the assertion.** A handler runs on a server task, outside any test's stack, so a
/// raise there had nowhere to land: it answered 500, recorded `error`, and was otherwise invisible.
/// A system under test with a retry or a fallback swallows that 500, and the suite goes green over a
/// broken handler — reporting prova's bug as the dependency's flakiness. Against the old engine this
/// file reports 6 passed / 0 failed.
///
/// **6 passed** is the other half: `allow_handler_errors` must actually opt out, and a healthy mock
/// must not fail its scope (without that negative control, every case here would pass against a mock
/// that failed unconditionally).
#[test]
fn mock_handler_errors_fail_the_owning_scope_unless_allowed() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("mock_handler_errors.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run mock_handler_errors.lua");
    assert_eq!(summary.passed, 6, "passed (bodies; allow_handler_errors opts out)");
    assert_eq!(summary.failed, 3, "failed (one teardown leaf per handler error)");
}
