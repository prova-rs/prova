//! End-to-end for init's generic **package-state injection** and the entry-level `in_package`
//! policy, through the real binary. The `arch-state` fixture echoes what it received (each state
//! answer defaults to the sentinel `absent`, so injection is distinguishable from its absence) —
//! this is the contract any archetype can rely on, not a plugin-specific side channel.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A fresh project dir plus an isolated XDG home with a catalog mapping `state` → the echo fixture
/// (as an augmenting entry) and `creator` → the same fixture under the default deny policy.
fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("prova-initstate-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();

    let cfg_dir = base.join("xdg/config/prova");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let config = format!(
        "[init.state]\n\
         description = \"state echo\"\n\
         source = '{src}'\n\
         in_package = \"allow\"\n\
         [init.creator]\n\
         description = \"state echo, never-clobber\"\n\
         source = '{src}'\n",
        src = fixture("arch-state").display(),
    );
    std::fs::write(cfg_dir.join("config.toml"), config).unwrap();
    (project, base.join("xdg"))
}

fn init(cwd: &Path, xdg: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .arg("init")
        .args(args)
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .output()
        .expect("run prova init")
}

fn cleanup(project: &Path) {
    if let Some(base) = project.parent() {
        std::fs::remove_dir_all(base).ok();
    }
}

fn state(cwd: &Path) -> String {
    std::fs::read_to_string(cwd.join("state.txt")).unwrap_or_default()
}

/// Outside any package: no switch, no answers — the fixture's `absent` sentinels survive.
#[test]
fn outside_a_package_no_state_is_injected() {
    let (project, xdg) = scratch("outside");
    let out = init(&project, &xdg, &["state", "--headless"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = state(&project);
    assert!(s.contains("in_package=no"), "switch leaked outside a package: {s}");
    assert!(s.contains("package_root=absent"), "{s}");
    assert!(s.contains("plugin_root=absent"), "{s}");
    cleanup(&project);
}

/// Inside a package at its root: the switch is on, the root is `.`, and the manifest's declared
/// `plugin_root` comes through verbatim.
#[test]
fn inside_a_package_state_reaches_the_archetype() {
    let (project, xdg) = scratch("inside");
    std::fs::write(
        project.join("prova.toml"),
        "[run]\nplugin_root = \".prova/plugins\"\n",
    )
    .unwrap();
    let out = init(&project, &xdg, &["state", "--headless"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = state(&project);
    assert!(s.contains("in_package=yes"), "{s}");
    assert!(s.contains("package_root=."), "{s}");
    assert!(s.contains("plugin_root=.prova/plugins"), "{s}");
    cleanup(&project);
}

/// From a subdirectory, the injected root is the RELATIVE path up to the package — an archetype can
/// compose `package_root/plugin_root/...` without knowing how deep it was invoked.
#[test]
fn a_subdirectory_reports_the_relative_package_root() {
    let (project, xdg) = scratch("subdir");
    std::fs::write(project.join("prova.toml"), "[run]\n").unwrap();
    let sub = project.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    let out = init(&sub, &xdg, &["state", "--headless"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = state(&sub);
    assert!(s.contains("package_root=../.."), "{s}");
    assert!(s.contains("plugin_root=absent"), "no plugin_root declared, none must be injected: {s}");
    cleanup(&project);
}

/// The default policy is unchanged never-clobber: a deny entry refuses an initialized directory.
#[test]
fn a_deny_entry_refuses_an_initialized_package() {
    let (project, xdg) = scratch("deny");
    std::fs::write(project.join("prova.toml"), "[run]\n").unwrap();
    let out = init(&project, &xdg, &["creator", "--headless"]);
    assert!(!out.status.success(), "deny entry rendered into an initialized package");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("already initialized"), "stderr: {stderr}");
    assert!(!project.join("state.txt").exists(), "guard failed open: state.txt was rendered");
    cleanup(&project);
}

/// Precedence: an explicit CLI `--answer` beats the injected state — the facts are lowest priority.
#[test]
fn cli_answers_override_injected_state() {
    let (project, xdg) = scratch("precedence");
    std::fs::write(project.join("prova.toml"), "[run]\n").unwrap();
    let out = init(
        &project,
        &xdg,
        &["state", "--headless", "--answer", "prova_package_root=elsewhere"],
    );
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let s = state(&project);
    assert!(s.contains("package_root=elsewhere"), "{s}");
    assert!(s.contains("in_package=yes"), "{s}");
    cleanup(&project);
}
