//! End-to-end for the `prova init` catalog through the real binary.
//!
//! `init` is a catalog over archetypes: prova embeds a base catalog (so `prova init` works with zero
//! user config) and `~/.config/prova/config.toml` layers `[init.*]` entries on top — a matching key
//! replaces the built-in, a new key adds one. These proofs drive the **non-interactive** surface
//! (`--list`, `init <key>`), which is the part that must stay deterministic; the `inquire` select is
//! proven only at its edges.
//!
//! Every run gets an isolated XDG home so the developer's real `~/.config/prova/config.toml` can
//! neither leak into a proof nor be written by one.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch, *uninitialized* project plus an isolated XDG home, both under one temp dir removed by
/// the caller. Returns `(project_dir, xdg_home)`.
fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("prova-initcat-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let xdg = base.join("xdg");
    std::fs::create_dir_all(xdg.join("config")).unwrap();
    (project, xdg)
}

fn init(project: &Path, xdg: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(project)
        .arg("init")
        .args(args)
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .output()
        .expect("run prova init")
}

/// Write `~/.config/prova/config.toml` inside the isolated XDG home.
fn write_config(xdg: &Path, toml: &str) {
    let dir = xdg.join("config").join("prova");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

fn cleanup(project: &Path) {
    if let Some(base) = project.parent() {
        std::fs::remove_dir_all(base).ok();
    }
}

/// Proof 5: with no user config at all, `--list` still prints the built-in `default` entry — key and
/// description — and exits 0 without prompting.
#[test]
fn list_prints_the_builtin_default() {
    let (project, xdg) = scratch("list");
    let out = init(&project, &xdg, &["--list"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("default"), "no `default` key in: {stdout}");
    // A key alone is not a catalog listing — the description is what makes it choosable.
    let line = stdout
        .lines()
        .find(|l| l.contains("default"))
        .unwrap_or_default();
    assert!(
        line.trim().len() > "default".len() + 4,
        "`default` listed without a description: {line:?}"
    );
    // `--list` is scriptable: it must not scaffold anything as a side effect.
    assert!(
        !project.join("prova.toml").exists() && !project.join("prova").exists(),
        "--list wrote to the project"
    );
    cleanup(&project);
}

/// Proof 5 (extension): a user-config entry joins the built-in in `--list`, and the built-in survives
/// alongside it — the merge is a union, not a replacement of the whole catalog.
#[test]
fn list_unions_user_entries_with_the_builtin() {
    let (project, xdg) = scratch("listuser");
    write_config(
        &xdg,
        "[init.service]\n\
         description = \"A service proof suite\"\n\
         source = \"/tmp/does-not-need-to-exist\"\n",
    );
    let out = init(&project, &xdg, &["--list"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("service"), "user entry missing: {stdout}");
    assert!(
        stdout.contains("A service proof suite"),
        "user description missing: {stdout}"
    );
    assert!(
        stdout.contains("default"),
        "built-in `default` was lost when user config added a key: {stdout}"
    );
    cleanup(&project);
}

/// Proof 13: an unknown key is a clear error that names the keys that *do* exist, and exits non-zero.
/// A scaffolder that silently does nothing (or renders the wrong thing) on a typo is worse than one
/// that refuses.
#[test]
fn unknown_key_errors_and_lists_the_available_keys() {
    let (project, xdg) = scratch("unknown");
    let out = init(&project, &xdg, &["bogus"]);
    assert!(
        !out.status.success(),
        "expected non-zero exit for an unknown catalog key"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("bogus"), "error omits the bad key: {stderr}");
    assert!(
        stderr.contains("default"),
        "error should list the available keys: {stderr}"
    );
    assert!(
        !project.join("prova.toml").exists() && !project.join("prova").exists(),
        "a failed init scaffolded anyway"
    );
    cleanup(&project);
}

/// A malformed `config.toml` is reported as a config error, not silently ignored. Silently falling
/// back to the built-in catalog would strand a user whose entries never appear.
#[test]
fn malformed_config_is_an_error_not_a_silent_fallback() {
    let (project, xdg) = scratch("badcfg");
    write_config(&xdg, "[init.broken\ndescription = ohno\n");
    let out = init(&project, &xdg, &["--list"]);
    assert!(!out.status.success(), "expected non-zero for broken config");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("config.toml"),
        "error should name the offending file: {stderr}"
    );
    cleanup(&project);
}
