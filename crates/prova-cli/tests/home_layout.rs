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

// ── generated state follows the home, not the manifest's own directory ────────────────────────
//
// The behavioural surface of `.prova/var/` is proven black-box in `proofs/layout/state_dir_test.lua`.
// What belongs *here* is the part that is purely about home resolution: state is keyed to the project
// ROOT, so every manifest variant and every invocation directory agree on one location. Getting this
// wrong would give one package two state dirs — and `--last-failed` would silently forget.

/// A package that always has something to record.
fn install_failing(root: &Path, manifest_rel: &str) {
    write(root, manifest_rel, "[run]\nproofs = [\"proofs\"]\n");
    write(
        root,
        "proofs/red_test.lua",
        "prova.test(\"red\", function(t) t:expect(1):equals(2) end)\n",
    );
}

/// All four manifest variants put generated state at `<root>/.prova/var/` — the nook-tucked manifest
/// included. `.prova/var/` is prova's, whether or not `.prova/` also holds the manifest.
#[test]
fn every_manifest_variant_records_state_at_the_root() {
    for (tag, manifest_rel) in [
        ("flat", "prova.toml"),
        ("flat-hidden", ".prova.toml"),
        ("nested", "prova/prova.toml"),
        ("nested-hidden", ".prova/prova.toml"),
    ] {
        let dir = tmp(&format!("state-{tag}"));
        install_failing(&dir, manifest_rel);
        let (ok, out) = run(&dir);
        assert!(!ok, "{tag}: the run must fail so there is state to record: {out}");

        assert!(
            dir.join(".prova/var/last-failed.json").is_file(),
            "{tag}: state belongs under <root>/.prova/var/"
        );
        assert!(
            dir.join(".prova/var/.gitignore").is_file(),
            "{tag}: the state dir must ignore itself"
        );
        // The whole point: nothing generated in the tracked tree.
        assert!(
            !dir.join("last-failed.json").exists() && !dir.join(".last-failed.json").exists(),
            "{tag}: nothing generated at the package root"
        );
    }
}

/// Run from the root, then from inside the nook: one home, so one state dir — and `--last-failed`
/// reads back what the other invocation wrote. Two locations here would mean a package silently
/// forgets its failures depending on which directory you happened to be standing in.
#[test]
fn state_is_shared_across_invocation_directories() {
    let dir = tmp("state-nook-agreement");
    install_failing(&dir, ".prova/prova.toml");

    let (ok, _) = run(&dir);
    assert!(!ok, "the root run must fail so there is state to record");
    assert!(dir.join(".prova/var/last-failed.json").is_file());

    // From inside the nook: home hoists to the parent, so no second state dir appears below it.
    let (ok, out) = run(&dir.join(".prova"));
    assert!(!ok, "the nook run must fail too: {out}");
    assert!(
        !dir.join(".prova/.prova").exists(),
        "a run from inside the nook must not nest a second state dir: {out}"
    );

    // And the record is live for the OTHER invocation directory — one shared piece of state.
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(dir.join(".prova"))
        .args(["--last-failed", "--json"])
        .output()
        .unwrap();
    let combined = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        combined.contains("\"failed\":1") && !combined.contains("running everything"),
        "--last-failed from the nook must select the root run's failure: {combined}"
    );
}

/// A relative `PROVA_VAR_DIR` is refused before anything runs. It would resolve against the cwd, and
/// prova runs from anywhere inside a package — so one setting would scatter state across directories,
/// which is precisely the inconsistency the escape hatch exists to avoid.
#[test]
fn a_relative_state_root_override_is_refused() {
    let dir = tmp("state-relative-override");
    install_failing(&dir, "prova.toml");

    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .env("PROVA_VAR_DIR", "relative/state")
        .output()
        .unwrap();
    let combined = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "must refuse: {combined}");
    assert!(
        combined.contains("PROVA_VAR_DIR") && combined.contains("absolute"),
        "the diagnostic must name the variable and the requirement: {combined}"
    );
    // Refused up front — before the suite ran, so no partial state either.
    assert!(
        !dir.join(".prova/var").exists() && !dir.join("relative").exists(),
        "a refused override must write nothing: {combined}"
    );
}
