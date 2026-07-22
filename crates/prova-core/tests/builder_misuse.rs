use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

fn testdata(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(file)
}

/// A bare `prova.test` inside a flow body registers at the file root — not as a step — so the
/// flow silently runs zero of "its" steps with no ordering and no cascade-skip. Collection must
/// refuse the bare form outright rather than accept the silently-wrong structure.
#[test]
fn bare_declaration_inside_a_builder_body_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("builder_bare_test.lua"), &mut reporter)
        .expect_err("a bare prova.test inside a flow body must fail collection");
    let msg = err.to_string();
    assert!(
        msg.contains("bare `prova.test`") && msg.contains("flow:step"),
        "the error must name the misuse and point at the builder, got: {msg}"
    );
}

/// A flow that declared no steps is never a real suite — it is the signature of ignoring the
/// builder argument, so it errors instead of passing vacuously.
#[test]
fn a_flow_with_no_steps_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("builder_empty_flow.lua"), &mut reporter)
        .expect_err("an empty flow must fail collection");
    let msg = err.to_string();
    assert!(
        msg.contains("declared no steps"),
        "the error must say the flow is empty, got: {msg}"
    );
}

/// Same contract for `prova.group`: zero children means the builder argument was ignored.
#[test]
fn a_group_with_no_children_is_an_error() {
    let mut reporter = NullReporter;
    let err = run_path(&testdata("builder_empty_group.lua"), &mut reporter)
        .expect_err("an empty group must fail collection");
    let msg = err.to_string();
    assert!(
        msg.contains("declared no children"),
        "the error must say the group is empty, got: {msg}"
    );
}
