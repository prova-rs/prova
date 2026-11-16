//! End-to-end (hermetic: no docker, no network): prova's exit code says whose fault a non-green run is, so a
//! caller like Herald can tell "this change is broken" from "this host lacks something" (E m/14201).
//!
//! - **0** green. **1** red: a proof failed.
//! - **2** an unmet guarantee: a `must_run` capability this host doesn't have. The host's fault; a caller paces it.
//! - **78** (sysexits `EX_CONFIG`) prova's own usage or configuration error: no `prova.toml` where one is needed, a
//!   manifest that doesn't read, an unknown profile, a bad argument. The change's fault; nothing on the host fixes it.

use std::path::Path;
use std::process::Command;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn prova(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .args(args)
        .env("XDG_CACHE_HOME", cwd.join(".x/cache"))
        .env("XDG_DATA_HOME", cwd.join(".x/data"))
        .env("XDG_CONFIG_HOME", cwd.join(".x/config"))
        .env_remove("PROVA_PROFILE")
        .env_remove("GITHUB_ACTIONS")
        .output()
        .expect("run prova")
}

fn code(out: &std::process::Output) -> i32 {
    out.status.code().expect("exited, not signalled")
}

fn said(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A scratch tree, removed on drop even when an assertion fails mid-test.
struct Tmp(std::path::PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("prova-exit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

const PASSING: &str = "prova.test(\"one is one\", function(t) t:expect(1):equals(1) end)\n";
const FAILING: &str = "prova.test(\"one is two\", function(t) t:expect(1):equals(2) end)\n";

/// A package whose proofs are `tests/`, with `manifest` as its prova.toml.
fn package(root: &Path, manifest: &str, proof: &str) {
    write(&root.join("prova.toml"), manifest);
    write(&root.join("tests/one_test.lua"), proof);
}

#[test]
fn no_prova_toml_in_manifest_mode_is_a_config_error() {
    let tmp = Tmp::new("none");
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 78, "{}", said(&out));
}

#[test]
fn a_malformed_prova_toml_is_a_config_error() {
    let tmp = Tmp::new("malformed");
    package(&tmp.0, "[run\nproofs = [\"tests\"\n", PASSING);
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 78, "{}", said(&out));
}

#[test]
fn an_unknown_profile_is_a_config_error() {
    let tmp = Tmp::new("profile");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", PASSING);
    let out = prova(&tmp.0, &["--profile", "no-such-profile"]);
    assert_eq!(code(&out), 78, "{}", said(&out));
}

#[test]
fn an_unknown_flag_is_a_config_error() {
    let tmp = Tmp::new("flag");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", PASSING);
    let out = prova(&tmp.0, &["--no-such-flag"]);
    assert_eq!(code(&out), 78, "{}", said(&out));
}

#[test]
fn an_unavailable_guaranteed_capability_is_the_hosts_and_exits_2() {
    let tmp = Tmp::new("must-run");
    package(
        &tmp.0,
        "[run]\nproofs = [\"tests\"]\nmust_run = [\"prova-no-such-capability\"]\n",
        PASSING,
    );
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

#[test]
fn a_failing_proof_is_red_and_exits_1() {
    let tmp = Tmp::new("red");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", FAILING);
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 1, "{}", said(&out));
}

#[test]
fn a_passing_proof_is_green_and_exits_0() {
    let tmp = Tmp::new("green");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", PASSING);
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 0, "{}", said(&out));
}

// E m/14323's rulings on the shared helpers: each split by cause, a row per side.

#[test]
fn a_package_source_that_names_both_path_and_git_is_a_config_error() {
    let tmp = Tmp::new("dep-both");
    package(
        &tmp.0,
        "[run]\nproofs = [\"tests\"]\n\n[dependencies]\nx = { path = \"a\", git = \"b\" }\n",
        PASSING,
    );
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 78, "{}", said(&out));
}

#[test]
fn a_package_fetch_that_fails_is_the_hosts_and_exits_2() {
    let tmp = Tmp::new("dep-fetch");
    let gone = tmp.0.join("no-such-repo");
    package(
        &tmp.0,
        &format!("[run]\nproofs = [\"tests\"]\n\n[dependencies]\nx = {{ git = \"file://{}\" }}\n", gone.display()),
        PASSING,
    );
    let out = prova(&tmp.0, &[]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

#[test]
fn a_resume_its_flags_contradict_is_a_config_error() {
    let tmp = Tmp::new("resume-flags");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", PASSING);
    let out = prova(&tmp.0, &["--resume", "-k", "one"]);
    assert_eq!(code(&out), 78, "{}", said(&out));
}

/// E m/14356: a resume the journal can't lend is the caller's ask not fitting the state (78), said so, never a 2 a
/// caller would pace and retry for a run that can never succeed.
#[test]
fn a_resume_with_nothing_journaled_is_the_callers_and_exits_78() {
    let tmp = Tmp::new("resume-state");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", PASSING);
    // A repository, so the tree fingerprints and the refusal is the journal's, not the fingerprint's.
    let git = |args: &[&str]| {
        let ok = Command::new("git").current_dir(&tmp.0).args(args).output().expect("git").status.success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["-c", "user.email=p@example.com", "-c", "user.name=p", "commit", "-qm", "a package"]);
    let out = prova(&tmp.0, &["--resume"]);
    assert_eq!(code(&out), 78, "{}", said(&out));
    assert!(said(&out).contains("nothing to resume") && said(&out).contains("run without --resume"), "{}", said(&out));
}

#[test]
fn a_proof_file_that_cannot_load_is_red_and_exits_1() {
    let tmp = Tmp::new("unloadable");
    package(&tmp.0, "[run]\nproofs = [\"tests\"]\n", "prova.test(\"broken\", function(t)\n");
    let out = prova(&tmp.0, &["--list"]);
    assert_eq!(code(&out), 1, "{}", said(&out));
}

fn planted(tag: &str, which: &str) -> std::process::Output {
    let tmp = Tmp::new(tag);
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&tmp.0)
        .arg("--version")
        .env("PROVA_TEST_PANIC", which)
        .env("XDG_CACHE_HOME", tmp.0.join(".x/cache"))
        .output()
        .expect("run prova")
}

/// E m/14379: a panic that unwinds out of main is prova's own bug: printed as before, said so, and exit 70
/// (EX_SOFTWARE), never Rust's 101.
#[test]
fn an_uncaught_panic_in_main_is_provas_own_and_exits_70_with_its_message() {
    let out = planted("panic-main", "main");
    assert_eq!(code(&out), 70, "{}", said(&out));
    assert!(said(&out).contains("a planted panic in main"), "the panic is printed as before: {}", said(&out));
    assert!(said(&out).contains("prova's own bug"), "{}", said(&out));
}

/// E m/14392: a panic someone joins on purpose stays theirs (archetect's render thread, a topology holder in MCP):
/// its site says so and prova goes on, never 70.
#[test]
fn a_panic_on_a_joined_thread_stays_its_sites_and_is_never_70() {
    let out = planted("panic-joined", "joined");
    assert_eq!(code(&out), 0, "{}", said(&out));
    assert!(said(&out).contains("its site says so and goes on"), "{}", said(&out));
    assert!(!said(&out).contains("prova's own bug"), "{}", said(&out));
}
