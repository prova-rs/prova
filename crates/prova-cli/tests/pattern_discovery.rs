use std::process::Command;

/// Name-pattern discovery: the bare pattern `proofs = ["proofs"]` finds every `proofs/` directory at
/// any depth — the multi-crate layout, from one manifest. (Cross-root *sharing* is a plugin now, via
/// `.prova/plugins/`, not `package.path` — so this proves discovery, not require.)
#[test]
fn name_pattern_discovers_proofs_dirs_at_any_depth() {
    let dir = std::env::temp_dir().join(format!("prova-pattern-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("proofs")).unwrap();
    std::fs::create_dir_all(dir.join("mod/proofs")).unwrap();
    std::fs::write(dir.join("prova.toml"), "[run]\nproofs = [\"proofs\"]\n").unwrap();
    std::fs::write(
        dir.join("proofs/a_test.lua"),
        "prova.test(\"root proof\", function(t) t:expect(1):equals(1) end)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("mod/proofs/b_test.lua"),
        "prova.test(\"nested proof\", function(t) t:expect(1):equals(1) end)\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .arg("--json")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"passed\":2") || stdout.contains("passed\": 2"),
        "both proofs at two depths discovered: {stdout}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
