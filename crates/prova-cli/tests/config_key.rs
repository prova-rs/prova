use std::process::Command;

/// The companion config file is resolvable — from the manifest `config` key, and overridable with
/// `--config` (and `PROVA_CONFIG`), chiefly so tests can point at a specific config.
///
/// The observable: a capability registered by the chosen config file makes a `requires`-gated test
/// RUN; the wrong (or default, absent) config leaves it unregistered and the test SKIPS. So this
/// asserts on run vs skip, end to end through the real binary.
fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("prova-config-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(d.join("proofs")).unwrap();
    std::fs::create_dir_all(d.join("setup")).unwrap();
    // A config file the manifest does NOT point at — only --config/PROVA_CONFIG reaches it.
    std::fs::write(
        d.join("setup/alt.lua"),
        "runtime.capability(\"alt_ready\", function() return true end)\n",
    )
    .unwrap();
    std::fs::write(
        d.join(".prova.toml"),
        "[run]\nproofs = [\"proofs\"]\n", // no `config` key: default would be prova.lua (absent)
    )
    .unwrap();
    std::fs::write(
        d.join("proofs/gated_test.lua"),
        "prova.test(\"needs alt\", { requires = { \"alt_ready\" } }, function(t) t:expect(1):equals(1) end)\n",
    )
    .unwrap();
    d
}

#[test]
fn config_flag_selects_the_companion() {
    let dir = scratch("flag");
    // Without --config: no companion (prova.lua absent), capability unregistered → the test SKIPS.
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .arg("--json")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"skipped\":1") || stdout.contains("skipped\": 1"),
        "default: gated test skips: {stdout}"
    );

    // With --config setup/alt.lua: capability registered → the test RUNS.
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .args(["--config", "setup/alt.lua", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"passed\":1") || stdout.contains("passed\": 1"),
        "--config: gated test runs: {stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn prova_config_env_selects_the_companion() {
    let dir = scratch("env");
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .env("PROVA_CONFIG", "setup/alt.lua")
        .arg("--json")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"passed\":1") || stdout.contains("passed\": 1"),
        "PROVA_CONFIG: gated test runs: {stdout}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
