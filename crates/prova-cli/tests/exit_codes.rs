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
