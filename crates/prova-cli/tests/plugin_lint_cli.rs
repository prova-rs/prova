//! `prova plugin lint` on a multi-file plugin: the entry's `require("<canonical>.<sub>")` must
//! resolve during lint (the CLI registers the plugin's namespace from its `prova-plugin.toml`).

use std::path::PathBuf;
use std::process::Command;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../prova-core/testdata/plugin_lint")
        .join(rel)
}

#[test]
fn lints_a_multi_file_plugin_that_requires_a_sibling() {
    let entry = fixture("multi/multi.lua");
    let output = Command::new(env!("CARGO_BIN_EXE_prova"))
        .args(["plugin", "lint", entry.to_str().unwrap()])
        .output()
        .expect("run prova");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "lint failed.\nstdout:\n{stdout}");
    assert!(stdout.contains("ok"), "stdout:\n{stdout}");
    assert!(stdout.contains("resource"), "stdout:\n{stdout}");
}
