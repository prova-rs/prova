//! `[run] proofs` — the discovery model: each entry is a directory-NAME pattern, and prova runs the
//! `*_test.lua` in every directory matching it anywhere below the prova home (default `["proofs"]`).
//! The home of a nested `.prova/prova.toml` (or `prova/prova.toml`) is the directory ABOVE it, so a
//! `proofs/` living at that root — with prova's own files tucked into `.prova/` — is discovered.
//!
//! Black-box, through the binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-disc-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `prova --list` from `cwd`; returns (success, stdout+stderr).
fn list(cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .arg("--list")
        .output()
        .unwrap();
    let combined =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), combined)
}

fn proof(name: &str) -> String {
    format!("prova.test(\"{name}\", function(t) t:expect(1):equals(1) end)\n")
}

/// The reported bug: a nested `.prova/prova.toml` roots the package at the directory ABOVE `.prova/`,
/// so a `proofs/` at that root is discovered (not looked for inside `.prova/`).
#[test]
fn nested_hidden_home_discovers_proofs_at_the_root() {
    let d = tmp("nested-root");
    write(&d, ".prova/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(&d, "proofs/alpha_test.lua", &proof("alpha"));
    let (ok, out) = list(&d);
    assert!(ok, "discovery should succeed: {out}");
    assert!(out.contains("alpha"), "the root proofs/ test is found: {out}");
}

/// The visible nested form behaves the same: `prova/prova.toml` roots at the parent.
#[test]
fn nested_visible_home_discovers_proofs_at_the_root() {
    let d = tmp("nested-visible");
    write(&d, "prova/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(&d, "proofs/beta_test.lua", &proof("beta"));
    let (ok, out) = list(&d);
    assert!(ok, "discovery should succeed: {out}");
    assert!(out.contains("beta"), "the root proofs/ test is found: {out}");
}

/// `proofs` is a name pattern found at ANY depth: a `proofs/` at the root and a `proofs/` nested under
/// a subdirectory are both discovered.
#[test]
fn proofs_dirs_are_found_at_any_depth() {
    let d = tmp("any-depth");
    write(&d, ".prova/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(&d, "proofs/root_test.lua", &proof("root_level"));
    write(&d, "services/orders/proofs/svc_test.lua", &proof("service_level"));
    let (ok, out) = list(&d);
    assert!(ok, "discovery should succeed: {out}");
    assert!(out.contains("root_level"), "root proofs/ found: {out}");
    assert!(out.contains("service_level"), "nested proofs/ found: {out}");
}

/// Omitting the key defaults to `["proofs"]`, so zero-config discovery finds `proofs/` dirs.
#[test]
fn proofs_defaults_when_key_omitted() {
    let d = tmp("default");
    write(&d, ".prova.toml", "[run]\n"); // flat, no `proofs` key
    write(&d, "proofs/gamma_test.lua", &proof("gamma"));
    let (ok, out) = list(&d);
    assert!(ok, "discovery should succeed: {out}");
    assert!(out.contains("gamma"), "default proofs/ discovery: {out}");
}

/// Prova's own nook is skipped: a plugin's self-test under `.prova/plugins/.../proofs/` is NOT run by
/// the consuming package, and neither are build dirs like `target/`.
#[test]
fn prova_nook_and_build_dirs_are_skipped() {
    let d = tmp("skips");
    write(&d, ".prova/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(&d, "proofs/real_test.lua", &proof("real_one"));
    // A plugin's own proofs, tucked in the nook — must not leak into this package's run.
    write(
        &d,
        ".prova/plugins/lib/proofs/plugin_test.lua",
        &proof("plugin_selftest"),
    );
    // A build artifact directory — never scanned.
    write(&d, "target/debug/proofs/junk_test.lua", &proof("build_junk"));
    let (ok, out) = list(&d);
    assert!(ok, "discovery should succeed: {out}");
    assert!(out.contains("real_one"), "the package's own proofs run: {out}");
    assert!(
        !out.contains("plugin_selftest"),
        "a plugin's proofs under .prova/ must NOT be discovered: {out}"
    );
    assert!(
        !out.contains("build_junk"),
        "target/ must NOT be scanned: {out}"
    );
}

/// Discovery stops at boundaries: a deeper directory that is its OWN package (has a manifest) is
/// independent — its proofs are not swept into this package — and a `testdata/` fixture tree is never
/// scanned.
#[test]
fn nested_package_and_testdata_are_excluded() {
    let d = tmp("boundaries");
    write(&d, ".prova/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(&d, "proofs/mine_test.lua", &proof("mine_own"));
    // A deeper, independent package — the nearest-manifest rule applies to discovery too.
    write(&d, "sub/prova.toml", "[run]\nproofs = [\"proofs\"]\n");
    write(&d, "sub/proofs/theirs_test.lua", &proof("their_own"));
    // A fixture tree — never a package's own proofs.
    write(&d, "testdata/fx/proofs/fixture_test.lua", &proof("just_a_fixture"));
    let (ok, out) = list(&d);
    assert!(ok, "discovery should succeed: {out}");
    assert!(out.contains("mine_own"), "own proofs found: {out}");
    assert!(
        !out.contains("their_own"),
        "a nested package's proofs are excluded: {out}"
    );
    assert!(
        !out.contains("just_a_fixture"),
        "testdata/ is skipped: {out}"
    );
}
