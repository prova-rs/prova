//! End-to-end for `prova init <key>` actually RENDERING the selected catalog archetype (M3), through
//! the real binary. Catalog entries point at the hermetic local fixtures under `tests/fixtures/`, so
//! nothing here touches the network. Renders drive the non-interactive paths (`--headless`,
//! `--answer`, `--switch`) since an interactive prompt needs a TTY.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Absolute path to a fixture archetype under this crate's `tests/fixtures/`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A fresh project dir (the cwd for `init`) plus an isolated XDG home whose `config/prova/config.toml`
/// maps catalog keys to the local fixtures. Returns `(project_dir, xdg_home)`.
fn scratch(tag: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("prova-initrender-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();

    let cfg_dir = base.join("xdg/config/prova");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // Three entries, each pointing at a local fixture archetype. `basic` also carries a baked answer
    // so the precedence test has something to override.
    // TOML literal (single-quoted) strings: Windows fixture paths carry
    // backslashes, which basic strings treat as escape sequences.
    let config = format!(
        "[init.basic]\n\
         description = \"basic fixture\"\n\
         source = '{basic}'\n\
         [init.basic.answers]\n\
         project_name = \"baked\"\n\
         [init.switched]\n\
         description = \"switch fixture\"\n\
         source = '{switched}'\n\
         [init.undef]\n\
         description = \"undefaulted fixture\"\n\
         source = '{undef}'\n",
        basic = fixture("arch-basic").display(),
        switched = fixture("arch-switched").display(),
        undef = fixture("arch-undefaulted").display(),
    );
    std::fs::write(cfg_dir.join("config.toml"), config).unwrap();
    (project, base.join("xdg"))
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

fn cleanup(project: &Path) {
    if let Some(base) = project.parent() {
        std::fs::remove_dir_all(base).ok();
    }
}

fn read(project: &Path, rel: &str) -> String {
    std::fs::read_to_string(project.join(rel)).unwrap_or_default()
}

/// Proof 6: `prova init basic --headless` renders the archetype into the cwd — a full prova project
/// (manifest + proof), exit 0 — and, IDE wiring being init's finishing step, writes `.luarc.json`.
#[test]
fn renders_the_selected_archetype_into_the_project() {
    let (project, xdg) = scratch("render");
    let out = init(&project, &xdg, &["basic", "--headless"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(project.join("prova.toml").is_file(), "manifest not rendered");
    assert!(project.join("README.md").is_file(), "README not rendered");
    assert!(
        project.join("proofs/example_test.lua").is_file(),
        "proof not rendered into the answered proof_dir"
    );
    // IDE wiring ran as the finishing step.
    assert!(project.join(".luarc.json").is_file(), "`.luarc.json` not written");
    cleanup(&project);
}

/// Proof 8: precedence — a CLI `--answer` overrides the entry's baked answer (`project_name=baked`).
#[test]
fn cli_answer_overrides_a_baked_answer() {
    let (project, xdg) = scratch("precedence");
    // Baseline: the baked answer flows through when no CLI answer is given.
    let out = init(&project, &xdg, &["basic", "--headless"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(read(&project, "README.md").contains("baked"), "baked answer not applied");
    cleanup(&project);

    // Override: CLI `--answer` wins.
    let (project, xdg) = scratch("precedence2");
    let out = init(&project, &xdg, &["basic", "--headless", "--answer", "project_name=fromcli"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let readme = read(&project, "README.md");
    assert!(readme.contains("fromcli"), "CLI answer did not override baked: {readme}");
    assert!(!readme.contains("baked"), "baked answer leaked through: {readme}");
    cleanup(&project);
}

/// Proof 11: `--switch` reaches the render — the gated file appears only when the switch is passed.
#[test]
fn switch_reaches_the_render() {
    // Without the switch: the gated file is absent.
    let (project, xdg) = scratch("noswitch");
    let out = init(&project, &xdg, &["switched", "--headless", "--no-luals"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!project.join("ci.yaml").exists(), "gated file rendered without the switch");
    cleanup(&project);

    // With `--switch ci`: it renders.
    let (project, xdg) = scratch("switch");
    let out = init(&project, &xdg, &["switched", "--headless", "--no-luals", "--switch", "ci"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(project.join("ci.yaml").is_file(), "switch did not reach the render");
    cleanup(&project);
}

/// Proof 14: `--headless` with an undefaulted, unanswered prompt fails cleanly (non-zero), never
/// hanging waiting for input.
#[test]
fn headless_errors_on_an_unanswerable_prompt() {
    let (project, xdg) = scratch("undef");
    let out = init(&project, &xdg, &["undef", "--headless", "--no-luals"]);
    assert!(!out.status.success(), "expected a non-zero exit on an unanswerable headless render");
    cleanup(&project);
}

/// Proof 15 (M6): a keyless `prova init` prompts an interactive select — but with no TTY (as here,
/// where the child's stdin isn't a terminal) it must fail clearly, naming `--list` / a key, rather
/// than hang waiting on a prompt it can never receive.
#[test]
fn keyless_without_a_tty_errors_clearly() {
    let (project, xdg) = scratch("notty");
    let out = init(&project, &xdg, &[]); // no key; the test child's stdin is not a terminal
    assert!(
        !out.status.success(),
        "keyless init without a terminal should not succeed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--list") || stderr.contains("terminal"),
        "error should guide the user to --list or a key: {stderr}"
    );
    // And nothing was scaffolded.
    assert!(!project.join("prova.toml").is_file(), "a project was scaffolded despite no selection");
    cleanup(&project);
}
