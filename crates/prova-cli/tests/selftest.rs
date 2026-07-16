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

    // Run only the suite's own top-level `*_test.lua` files. `fixtures/` holds inner projects the
    // self-tests drive `prova` against — including a deliberately-red test file (the MCP fixture) —
    // so the outer run must not recurse into it.
    let mut files: Vec<PathBuf> = std::fs::read_dir(&selftest)
        .expect("read selftest dir")
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with("_test.lua"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no *_test.lua files found in {}", selftest.display());

    let output = Command::new(bin)
        .args(&files)
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
