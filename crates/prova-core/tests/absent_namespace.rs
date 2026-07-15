//! The call-side companion to the `requires` skip: in a build that lacks a native feature, its
//! namespace is an absent-stub whose field access raises a clear "not compiled into this build"
//! error instead of a bare `attempt to index a nil value`.
//!
//! This whole file compiles/runs only when the `kafka` feature is OFF, so exercise it with
//! `cargo test -p prova-core --no-default-features --test absent_namespace`. In the default
//! (all-features) build there is nothing to test — `kafka` is the real namespace — so it is skipped.
#![cfg(not(feature = "kafka"))]

use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

#[test]
fn absent_namespace_raises_clear_error() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/absent_namespace.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run absent_namespace.lua");
    assert_eq!(summary.passed, 1, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
