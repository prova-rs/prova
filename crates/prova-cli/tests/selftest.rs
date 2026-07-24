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
    assert!(
        !files.is_empty(),
        "no *_test.lua files found in {}",
        selftest.display()
    );

    // Point the whole tree (this run and every inner `prova` it spawns, which inherit the env) at a
    // throwaway XDG home, so `cargo test` never writes to the developer's real `~/.cache/prova`.
    // Safe to isolate because nothing in the selftest's package environment declares a git plugin:
    // selftest/prova.toml is the hermeticity barrier that keeps explicit-path package discovery
    // from walking up to the repo root's .prova.toml (whose pinned git plugins would otherwise be
    // fetched over the network — flaky offline, and outright broken on Windows runners).
    let sandbox = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("selftest-xdg");
    let output = Command::new(bin)
        .args(&files)
        .env("PROVA_BIN", bin)
        .env("PROVA_FIXTURES", &fixtures)
        .env("XDG_CACHE_HOME", sandbox.join("cache"))
        .env("XDG_DATA_HOME", sandbox.join("data"))
        .env("XDG_CONFIG_HOME", sandbox.join("config"))
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
