//! End-to-end for `prova ide setup` through the real binary — the verb split out of `prova init`.
//!
//! `ide setup` is the re-runnable IDE-wiring half: install the shared LuaLS core stubs (under the
//! cache annotations dir, keyed by version) and create/merge the project's `.luarc.json` pointer,
//! per `--manage`. Every test runs against an isolated XDG home so the real `~/.cache/prova` is
//! never touched and the annotations dir is checkable.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch project (a `.prova.toml` at the root) plus an isolated XDG home, both under one temp dir
/// removed by the caller. Returns `(project_dir, xdg_home)`.
fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("prova-ide-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(project.join("proofs")).unwrap();
    std::fs::write(project.join(".prova.toml"), "[run]\npaths = [\"proofs\"]\n").unwrap();
    let xdg = base.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    (project, xdg)
}

fn ide_setup(project: &Path, xdg: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(project)
        .arg("ide")
        .arg("setup")
        .args(args)
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .output()
        .expect("run prova ide setup")
}

fn cleanup(project: &Path) {
    // The temp base is the project's grandparent (base/project); remove the whole base.
    if let Some(base) = project.parent() {
        std::fs::remove_dir_all(base).ok();
    }
}

/// The core annotation dir for a version is `<cache>/prova/annotations/<version>/`. Prova stamps its
/// own version; find whatever single version dir was written.
fn core_stub_exists(xdg: &Path) -> bool {
    let anno = xdg.join("cache").join("prova").join("annotations");
    let Ok(entries) = std::fs::read_dir(&anno) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .any(|e| e.path().join("prova.lua").is_file())
}

/// A scratch project with a NESTED manifest (`.prova/prova.toml`), plus an isolated XDG home. The
/// project dir is the editor root; `.prova/` is the prova home under it.
fn scratch_nested(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("prova-ide-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(project.join(".prova")).unwrap();
    std::fs::create_dir_all(project.join("proofs")).unwrap();
    std::fs::write(
        project.join(".prova/prova.toml"),
        "[run]\npaths = [\"../proofs\"]\n",
    )
    .unwrap();
    let xdg = base.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    (project, xdg)
}

/// `.luarc.json` goes at the EDITOR root — the directory you open as a workspace — not inside the
/// prova home. For a nested `.prova/prova.toml` that means the project dir (the parent of `.prova/`),
/// so LuaLS, which binds to the workspace root, actually finds it. Path resolution is unaffected;
/// only the editor pointer follows the editor root.
#[test]
fn nested_home_writes_luarc_at_the_editor_root_not_inside_prova() {
    let (project, xdg) = scratch_nested("nested");
    let out = ide_setup(&project, &xdg, &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        project.join(".luarc.json").is_file(),
        "`.luarc.json` should sit at the project (editor) root, beside `.prova/`"
    );
    assert!(
        !project.join(".prova/.luarc.json").exists(),
        "`.luarc.json` must NOT be written inside the `.prova/` home dir"
    );
    cleanup(&project);
}

/// Proof 1: in a project with a manifest, `ide setup` writes `.luarc.json` and exits 0.
#[test]
fn writes_luarc_and_installs_stubs() {
    let (project, xdg) = scratch("basic");
    let out = ide_setup(&project, &xdg, &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let luarc = project.join(".luarc.json");
    assert!(luarc.is_file(), "`.luarc.json` was not created");
    let text = std::fs::read_to_string(&luarc).unwrap();
    assert!(text.contains("workspace.library"), "{text}");
    assert!(core_stub_exists(&xdg), "core stubs were not installed");
    cleanup(&project);
}

/// Proof 2: `ide setup` is idempotent — a second run leaves exactly one core-stub entry, not two.
#[test]
fn idempotent_second_run() {
    let (project, xdg) = scratch("idem");
    assert!(ide_setup(&project, &xdg, &[]).status.success());
    assert!(ide_setup(&project, &xdg, &[]).status.success());

    let text = std::fs::read_to_string(project.join(".luarc.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let lib = doc["workspace.library"].as_array().unwrap();
    // The core annotations dir appears once (path contains ".../annotations/<version>").
    let core_entries = lib
        .iter()
        .filter(|v| v.as_str().is_some_and(|s| s.contains("annotations")))
        .count();
    assert_eq!(
        core_entries, 1,
        "duplicate core entry after re-run: {lib:?}"
    );
    cleanup(&project);
}

/// Proof 3: `--manage never` installs the stubs but writes no `.luarc.json`.
#[test]
fn manage_never_installs_stubs_but_no_luarc() {
    let (project, xdg) = scratch("never");
    let out = ide_setup(&project, &xdg, &["--manage", "never"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !project.join(".luarc.json").exists(),
        "`.luarc.json` should not exist under --manage never"
    );
    assert!(
        core_stub_exists(&xdg),
        "stubs should still be installed under --manage never"
    );
    cleanup(&project);
}

/// Proof 4: outside any prova project (no manifest up the tree), `ide setup` fails with guidance
/// rather than silently doing nothing.
#[test]
fn errors_without_a_project() {
    let base = std::env::temp_dir().join(format!("prova-ide-noproj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&base)
        .args(["ide", "setup"])
        .env("XDG_CACHE_HOME", base.join("cache"))
        .env("XDG_DATA_HOME", base.join("data"))
        .env("XDG_CONFIG_HOME", base.join("config"))
        .output()
        .expect("run prova ide setup");
    assert!(
        !out.status.success(),
        "expected failure with no manifest in the tree"
    );
    std::fs::remove_dir_all(&base).ok();
}
