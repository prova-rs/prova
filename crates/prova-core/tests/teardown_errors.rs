use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// Teardown errors are reported, not swallowed — `docs/design/api.md` §Open questions #2, resolved
/// in favour of *separate leaves*.
///
/// Before this, `teardown_scope` did `let _ = f.call_async(...)`. That is why the counts below are
/// the whole test: **2 failed** is the assertion. A cleanup that raised used to be discarded, so
/// `ctx:manage` failing to stop a container was a leak the run reported as green. Run this file
/// against the old engine and it reports 2 passed / 0 failed.
///
/// **3 passed** matters just as much, and is the subtler half. The flow's second step must RUN:
/// a teardown failure must not cascade-skip a flow's later steps, nor gate a `depends_on`
/// dependent — the body passed; only its cleanup raised. The first draft of this change keyed the
/// cascade on "any failed result" and skipped that step, which is what the proof caught.
#[test]
fn teardown_errors_are_reported_and_do_not_gate() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("teardown_errors.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run teardown_errors.lua");
    assert_eq!(summary.passed, 3, "passed (the flow's 2nd step must not be skipped)");
    assert_eq!(summary.failed, 2, "failed (one teardown leaf per raising cleanup)");
    assert_eq!(summary.skipped, 0, "skipped");
}
