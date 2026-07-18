use std::process::Command;

/// With a `.prova/` home, test `paths` resolve against the project ROOT, not the home dir — so
/// `proofs/` lives at the root (a sibling of `.prova/`) while `.prova/` holds prova's config/plugins.
///
/// RED today: `paths` resolve against `home.dir` (`.prova/`), so `paths = ["proofs"]` looks for
/// `.prova/proofs` and finds nothing.
#[test]
fn paths_resolve_against_the_project_root() {
    let dir = std::env::temp_dir().join(format!("prova-rootpath-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".prova")).unwrap();
    std::fs::create_dir_all(dir.join("proofs")).unwrap();
    std::fs::write(dir.join(".prova/prova.toml"), "[run]\npaths = [\"proofs\"]\n").unwrap();
    std::fs::write(
        dir.join("proofs/root_test.lua"),
        "prova.test(\"proofs at the root\", function(t) t:expect(1):equals(1) end)\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .arg("--json")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("\"passed\":1") || stdout.contains("passed\": 1"),
        "proofs/ at the root is discovered from a .prova/ home: out={stdout} err={stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
