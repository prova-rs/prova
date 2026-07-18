use std::path::PathBuf;

use prova_core::{run_path, run_path_with, watch, NullReporter, PortMode, RunConfig};

fn testdata(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(file)
}

/// A `prova.topology` is usable in test mode exactly like a fixture (`t:use`), and defaults to
/// `Scope.File` — provisioned once and shared across the file's tests. Both tests pass, and the
/// second observing `built == 1` proves the single shared instantiation. (`prova up`'s held-execution
/// path is exercised manually / in CLI smoke tests, since it blocks on a signal.)
#[test]
fn topology_is_usable_as_a_file_scoped_fixture() {
    let mut reporter = NullReporter;
    let summary = run_path(&testdata("topology.lua"), &mut reporter).expect("run topology.lua");
    assert_eq!(summary.passed, 2, "both topology tests pass");
    assert_eq!(summary.failed, 0, "no failures");
}

/// The port mode set on `RunConfig` (`Auto` for tests, `Fixed` for `prova up --fixed`) reaches Lua as
/// `prova.ports`, so a topology definition can stay identical across verbs while `prova.containerized`
/// (and advertised-listener recipes) adapt their port strategy. Drives one file under both modes.
#[test]
fn port_mode_is_exposed_to_lua_as_prova_ports() {
    for (mode, expected) in [(PortMode::Auto, "auto"), (PortMode::Fixed, "fixed")] {
        // The file asserts `prova.ports == os.getenv("EXPECTED_PORTS")`; set the expectation to the
        // mode we configured. (`os.getenv` is routed through Rust's env in `build_lua`.)
        std::env::set_var("EXPECTED_PORTS", expected);
        let config = RunConfig::new(1).with_ports(mode);
        let mut reporter = NullReporter;
        let summary = run_path_with(&testdata("port_mode.lua"), &mut reporter, &config)
            .expect("run port_mode.lua");
        assert_eq!(summary.passed, 1, "{mode:?}: prova.ports == {expected:?}");
        assert_eq!(summary.failed, 0, "{mode:?}: no failures");
    }
    std::env::remove_var("EXPECTED_PORTS");
}

/// `watch` on a topology that does not exist fails fast with a helpful error — it never enters the
/// hold loop (so this test can't hang) and never calls the ready/error callbacks. The happy path
/// (provision → re-apply on file change → teardown) blocks on a signal and is verified via the CLI.
#[test]
fn watch_errors_immediately_on_unknown_topology() {
    let files = [testdata("topology.lua")]; // defines "web", not "nope"
    let config = RunConfig::new(1);
    let mut ready_calls = 0;
    let mut error_calls = 0;
    let result = watch(
        &files,
        "nope",
        &config,
        |_, _| ready_calls += 1,
        |_| error_calls += 1,
    );
    let err = result.expect_err("unknown topology is an error");
    assert!(
        err.to_string().contains("no topology named"),
        "helpful message, got: {err}"
    );
    assert_eq!(ready_calls, 0, "never provisioned");
    assert_eq!(
        error_calls, 0,
        "load failure is returned, not sent to on_error"
    );
}
