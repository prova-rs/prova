use std::path::PathBuf;
use std::process::Command;

/// Ultimate dogfooding: run the real `prova` binary against prova's own self-test suite, which in
/// turn invokes `prova` against inner fixtures and asserts on exit codes + output. This gives
/// black-box acceptance coverage of the assembled CLI (arg parsing, discovery, reporting, the
/// manifest) that the library-level tests can't reach. Needs no external services.
#[test]
fn prova_acceptance_tests_itself() {
    let bin = env!("CARGO_BIN_EXE_prova");
    let selftest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("selftest");
    let fixtures = selftest.join("fixtures");

    let output = Command::new(bin)
        .arg(&selftest)
        .env("PROVA_BIN", bin)
        .env("PROVA_FIXTURES", &fixtures)
        .output()
        .expect("run prova on its own self-test suite");

    if !output.status.success() {
        eprintln!(
            "--- prova self-test stdout ---\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        eprintln!(
            "--- prova self-test stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(
        output.status.success(),
        "prova's self-test suite must pass (prova testing prova)"
    );
}
