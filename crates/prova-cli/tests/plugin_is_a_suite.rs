//! The manifests are one: a single `prova.toml` can be BOTH a requireable plugin (`[plugin]`) and a
//! runnable test suite (`[run]`). There is no `prova-plugin.toml` — a plugin is a project, and a
//! project can publish itself as a plugin. This is the executable form of "no difference between a
//! plugin and a test suite": the topology a plugin ships is proven by the suite in the same manifest.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-pis-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn run(cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .arg("--json")
        .output()
        .unwrap();
    let combined =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), combined)
}

// `mylib` — ONE `prova.toml`, two faces. Its entry is a NON-conventional path (`src/mylib.lua`), so a
// consumer can only resolve it if the searcher read `[plugin] entry` from `prova.toml` — the merge.
fn build_mylib(base: &Path) {
    write(
        base,
        "mylib/prova.toml",
        "[plugin]\nname = \"mylib\"\nentry = \"src/mylib.lua\"\n\n[run]\nproofs = [\"proofs\"]\n",
    );
    write(base, "mylib/src/mylib.lua", "return { answer = 42 }\n");
    write(
        base,
        "mylib/proofs/self_test.lua",
        "prova.test(\"mylib proves itself\", function(t) t:expect(1):equals(1) end)\n",
    );
}

/// One `prova.toml` is both runnable as its own suite AND requireable as a plugin.
///
/// RED before the merge: the plugin's metadata lived in `prova-plugin.toml`, so with only a
/// `prova.toml` the searcher never learns the non-conventional entry and `require("mylib")` fails.
#[test]
fn one_manifest_is_both_a_requireable_plugin_and_a_runnable_suite() {
    let base = tmp("both");
    build_mylib(&base);

    // (a) The suite face: `prova` in mylib/ runs its own proofs — the `[plugin]` section doesn't
    // interfere with being a runnable project.
    let (ok, out) = run(&base.join("mylib"));
    assert!(
        ok && out.contains("\"passed\":1"),
        "mylib runs its own proofs: {out}"
    );

    // (b) The plugin face: a consumer declares mylib and require()s it. Resolving the non-conventional
    // `src/mylib.lua` proves the searcher read `prova.toml [plugin] entry`.
    write(
        &base,
        "app/prova.toml",
        "[run]\nproofs = [\"proofs\"]\n\n[plugins]\nmylib = { path = \"../mylib\" }\n",
    );
    write(
        &base,
        "app/proofs/uses_test.lua",
        "local m = require(\"mylib\")\nprova.test(\"consumer uses mylib\", function(t) t:expect(m.answer):equals(42) end)\n",
    );
    let (ok, out) = run(&base.join("app"));
    assert!(
        ok && out.contains("\"passed\":1"),
        "consumer require(mylib) resolves via prova.toml [plugin]: {out}"
    );
}
