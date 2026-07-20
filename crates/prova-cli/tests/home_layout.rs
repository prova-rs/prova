//! Home discovery + resolution: the manifest's directory is the base for EVERYTHING, so root and
//! home are one thing. Black-box, through the binary.
//!
//! The headline property is relocatability: a prova project is a self-contained unit. Move its
//! manifest — from a nested `prova/` up to the project root — with the files it references, and the
//! manifest itself does not change a byte, because every path in it (`paths`, `config`,
//! `plugin_root`) resolves against the manifest's own directory.

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

// One manifest, three home-relative keys — the whole point is that these bytes are identical whether
// the manifest sits in `prova/` or at the root.
const MANIFEST: &str = "\
[run]
paths = [\"proofs\"]
config = \"config.lua\"
plugin_root = \"plugins\"
";
const CONFIG: &str = "runtime.capability(\"wired\", function() return true end)\n";
const PLUGIN: &str = "return { answer = 42 }\n";
// Exercises all three keys at once: `paths` found this file, `config` registered `wired` (so the
// `requires` gate passes rather than skipping), and `plugin_root` resolved `require(\"helper\")`.
const PROOF: &str = "\
local helper = require(\"helper\")
prova.test(\"everything resolves against the manifest dir\", { requires = { \"wired\" } }, function(t)
  t:expect(helper.answer):equals(42)
end)
";

fn install(root: &Path, prefix: &str) {
    let at = |rel: &str| {
        if prefix.is_empty() {
            rel.to_string()
        } else {
            format!("{prefix}/{rel}")
        }
    };
    write(root, &at("prova.toml"), MANIFEST);
    write(root, &at("config.lua"), CONFIG);
    write(root, &at("plugins/helper/init.lua"), PLUGIN);
    write(root, &at("proofs/x_test.lua"), PROOF);
}

/// The same manifest bytes resolve whether the project lives in a nested `prova/` or flat at the
/// root. RED before root/home unify: with a nested `prova/prova.toml`, `paths` resolves against the
/// parent (the old "root"), so `proofs/` under `prova/` is never found.
#[test]
fn a_project_relocates_without_editing_its_manifest() {
    let nested = tmp("relocate-nested");
    install(&nested, "prova");
    let (ok, out) = run(&nested);
    assert!(ok && out.contains("\"passed\":1"), "nested prova/: {out}");

    let flat = tmp("relocate-flat");
    install(&flat, "");
    let (ok, out) = run(&flat);
    assert!(ok && out.contains("\"passed\":1"), "flat root: {out}");
}

/// Discovery is stable no matter where inside the project prova runs — from the nested home dir
/// itself, resolution is identical to running from the parent.
#[test]
fn discovery_is_stable_from_inside_the_home_dir() {
    let dir = tmp("relocate-inside");
    install(&dir, "prova");
    let (ok, out) = run(&dir.join("prova"));
    assert!(
        ok && out.contains("\"passed\":1"),
        "cd prova && prova: {out}"
    );
}

/// Exactly one of the four manifest variants may sit in a single directory. Two is an ambiguous
/// layout prova refuses to guess at.
#[test]
fn two_variants_in_one_dir_is_ambiguous() {
    let dir = tmp("ambiguous");
    write(&dir, "prova.toml", MANIFEST);
    write(&dir, ".prova/prova.toml", MANIFEST);
    let (ok, out) = run(&dir);
    assert!(!ok, "ambiguous layout must fail: {out}");
    assert!(out.contains("ambiguous"), "names the problem: {out}");
}

/// A manifest deeper in the tree is its OWN project — not an ambiguity with an ancestor's manifest,
/// and not merged into it. Running from the child resolves the child; the parent's suite never runs.
#[test]
fn a_deeper_manifest_is_an_independent_project() {
    let dir = tmp("nested-projects");
    // Parent project: a test that FAILS, so we can tell if it ever runs.
    write(&dir, "prova.toml", "[run]\npaths = [\"proofs\"]\n");
    write(
        &dir,
        "proofs/parent_test.lua",
        "prova.test(\"PARENT\", function(t) t:expect(1):equals(2) end)\n",
    );
    // Child project in a subdir: a passing test.
    write(&dir, "sub/prova.toml", "[run]\npaths = [\"proofs\"]\n");
    write(
        &dir,
        "sub/proofs/child_test.lua",
        "prova.test(\"child\", function(t) t:expect(1):equals(1) end)\n",
    );

    let (ok, out) = run(&dir.join("sub"));
    assert!(
        ok && out.contains("\"passed\":1") && out.contains("\"failed\":0"),
        "child project runs, parent does not: {out}"
    );
    assert!(!out.contains("PARENT"), "parent suite must not run: {out}");
}
