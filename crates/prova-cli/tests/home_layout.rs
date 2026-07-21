//! Home discovery + resolution: `home.dir` is the project ROOT, and every manifest-relative key
//! (`proofs`, `config`, `plugin_root`) resolves against it — whether the manifest sits flat at the
//! root or is tucked into a `prova/` / `.prova/` nook. Black-box, through the binary.
//!
//! The headline property: the nested form lets a package hide prova's own files (the manifest,
//! `config.lua`, `plugins/`) inside `.prova/` while the ROOT — where `proofs/` live and where an
//! editor attaches — stays the parent. So a flat and a nested layout resolve the SAME root; only the
//! `config`/`plugin_root` paths differ (they point into the nook for the nested one).

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-home-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Run `prova --json` from `cwd`; return (success, stdout+stderr).
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

const CONFIG: &str = "runtime.capability(\"wired\", function() return true end)\n";
const PLUGIN: &str = "return { answer = 42 }\n";
// Exercises all three keys at once: `proofs` discovered this file, `config` registered `wired` (so
// the `requires` gate passes rather than skipping), and `plugin_root` resolved `require(\"helper\")`.
const PROOF: &str = "\
local helper = require(\"helper\")
prova.test(\"everything resolves from the package root\", { requires = { \"wired\" } }, function(t)
  t:expect(helper.answer):equals(42)
end)
";

// Flat: the manifest and prova's files all live at the root, referenced by bare names.
const FLAT_MANIFEST: &str = "\
[run]
proofs = [\"proofs\"]
config = \"config.lua\"
plugin_root = \"plugins\"
";
// Nested: the manifest and prova's files tuck into `.prova/`; the three keys point INTO the nook, all
// relative to the ROOT. `proofs/` stays at the root, in the open.
const NESTED_MANIFEST: &str = "\
[run]
proofs = [\"proofs\"]
config = \".prova/config.lua\"
plugin_root = \".prova/plugins\"
";

fn install_flat(root: &Path) {
    write(root, "prova.toml", FLAT_MANIFEST);
    write(root, "config.lua", CONFIG);
    write(root, "plugins/helper/init.lua", PLUGIN);
    write(root, "proofs/x_test.lua", PROOF);
}

fn install_nested(root: &Path) {
    write(root, ".prova/prova.toml", NESTED_MANIFEST);
    write(root, ".prova/config.lua", CONFIG);
    write(root, ".prova/plugins/helper/init.lua", PLUGIN);
    write(root, "proofs/x_test.lua", PROOF); // at the ROOT, visible
}

/// The project root is the home whether the manifest is flat at the root or tucked into `.prova/`.
/// Both discover `proofs/` at the root and resolve `config`/`plugin_root` from the root.
#[test]
fn flat_and_nested_both_root_at_the_package_root() {
    let flat = tmp("relocate-flat");
    install_flat(&flat);
    let (ok, out) = run(&flat);
    assert!(ok && out.contains("\"passed\":1"), "flat root: {out}");

    let nested = tmp("relocate-nested");
    install_nested(&nested);
    let (ok, out) = run(&nested);
    assert!(ok && out.contains("\"passed\":1"), "nested .prova/: {out}");
}

/// Discovery is stable from inside the `.prova/` nook: home resolves to the parent (the root), so the
/// same suite runs — `prova` works from anywhere inside the package, including the nook itself.
#[test]
fn discovery_is_stable_from_inside_the_nook() {
    let dir = tmp("relocate-inside");
    install_nested(&dir);
    let (ok, out) = run(&dir.join(".prova"));
    assert!(ok && out.contains("\"passed\":1"), "cd .prova && prova: {out}");
}

/// Exactly one of the four manifest variants may sit in a single directory. Two is an ambiguous
/// layout prova refuses to guess at — both would root at the same directory.
#[test]
fn two_variants_in_one_dir_is_ambiguous() {
    let dir = tmp("ambiguous");
    write(&dir, "prova.toml", FLAT_MANIFEST);
    write(&dir, ".prova/prova.toml", NESTED_MANIFEST);
    let (ok, out) = run(&dir);
    assert!(!ok, "ambiguous layout must fail: {out}");
    assert!(out.contains("ambiguous"), "names the problem: {out}");
}

/// A manifest deeper in the tree is its OWN package — not an ambiguity with an ancestor's manifest,
/// and not merged into it. Running from the child resolves the child; the parent's suite never runs.
#[test]
fn a_deeper_manifest_is_an_independent_package() {
    let dir = tmp("nested-packages");
    // Parent package: a test that FAILS, so we can tell if it ever runs.
    write(&dir, "prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(
        &dir,
        "proofs/parent_test.lua",
        "prova.test(\"PARENT\", function(t) t:expect(1):equals(2) end)\n",
    );
    // Child package in a subdir: a passing test.
    write(&dir, "sub/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(
        &dir,
        "sub/proofs/child_test.lua",
        "prova.test(\"child\", function(t) t:expect(1):equals(1) end)\n",
    );

    let (ok, out) = run(&dir.join("sub"));
    assert!(
        ok && out.contains("\"passed\":1") && out.contains("\"failed\":0"),
        "child package runs, parent does not: {out}"
    );
    assert!(!out.contains("PARENT"), "parent suite must not run: {out}");
}
