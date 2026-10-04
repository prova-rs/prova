//! End-to-end proofs of the run-scoped dependency override (`prova --dep`, `PROVA_DEP_<NAME>`): a
//! consumer pinned to one tag of a git package is judged against another for ONE run, with no file
//! touched and the override named on stderr and in the run record (gap 01a103f0: the integration
//! train's release gate diffs a consumer's verdict under the old tag and the new one).

use std::path::Path;
use std::process::{Command, Output};

fn git(args: &[&str], cwd: &Path) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "prova")
        .env("GIT_AUTHOR_EMAIL", "prova@example.com")
        .env("GIT_COMMITTER_NAME", "prova")
        .env("GIT_COMMITTER_EMAIL", "prova@example.com")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn write_package(remote: &Path, tag: &str) {
    std::fs::write(
        remote.join("greet.lua"),
        format!("local greet = {{}}\nfunction greet.tag() return \"{tag}\" end\nreturn greet\n"),
    )
    .unwrap();
}

/// A remote package with two tags: v1 answers "one", v2 answers "two".
fn init_remote(remote: &Path) {
    std::fs::create_dir_all(remote).unwrap();
    write_package(remote, "one");
    git(&["init", "-q", "-b", "main"], remote);
    git(&["add", "."], remote);
    git(&["commit", "-q", "-m", "one"], remote);
    git(&["tag", "v1"], remote);
    write_package(remote, "two");
    git(&["add", "."], remote);
    git(&["commit", "-q", "-m", "two"], remote);
    git(&["tag", "v2"], remote);
}

/// A consumer PINNED to v1 whose test expects "two": it passes only when judged against v2.
fn write_project(project: &Path, remote: &Path) -> String {
    std::fs::create_dir_all(project.join("tests")).unwrap();
    let manifest = format!(
        "[run]\nproofs = [\"tests\"]\n\n[dependencies]\ngreet = {{ git = \"{}\", tag = \"v1\" }}\n",
        remote.to_string_lossy().replace('\\', "/"),
    );
    std::fs::write(project.join("prova.toml"), &manifest).unwrap();
    std::fs::write(
        project.join("tests").join("greet_test.lua"),
        "local greet = require(\"greet\")\nprova.test(\"judged against v2\", function(t)\n  t:expect(greet.tag()):equals(\"two\")\nend)\n",
    )
    .unwrap();
    manifest
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("prova-dep-override-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn run_prova(project: &Path, home: &Path, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prova"));
    cmd.current_dir(project)
        .args(extra)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"));
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run prova")
}

fn record_overrides(project: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(project.join(".prova/var/last-run.json"))
        .or_else(|_| std::fs::read_to_string(project.join("var/last-run.json")))
        .expect("a run record");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json record");
    v["dependency_overrides"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[cfg_attr(windows, ignore = "local-path git fetch hits ERROR_ACCESS_DENIED on Windows CI runners")]
#[test]
fn a_consumer_is_judged_against_another_tag_for_one_run_and_the_record_says_so() {
    let root = scratch("cli");
    let (remote, project, home) = (root.join("remote"), root.join("project"), root.join("home"));
    init_remote(&remote);
    let manifest = write_project(&project, &remote);

    let pinned = run_prova(&project, &home, &[], &[]);
    assert!(!pinned.status.success(), "the manifest's v1 answers \"one\": red");

    let over = run_prova(&project, &home, &["--dep", "greet=v2"], &[]);
    let err = String::from_utf8_lossy(&over.stderr);
    assert!(over.status.success(), "judged against v2: green\n{err}");
    assert!(
        err.contains("dependency override, this run only: greet: tag v1 -> tag v2 (--dep)"),
        "named on stderr\n{err}"
    );
    assert_eq!(
        record_overrides(&project),
        vec!["greet: tag v1 -> tag v2 (--dep)".to_string()],
        "and in the run record"
    );
    assert_eq!(std::fs::read_to_string(project.join("prova.toml")).unwrap(), manifest, "no file touched");

    let again = run_prova(&project, &home, &[], &[]);
    assert!(!again.status.success(), "the next plain run is the manifest's v1 again");
    assert!(record_overrides(&project).is_empty(), "and its record names no override");
}

#[cfg_attr(windows, ignore = "local-path git fetch hits ERROR_ACCESS_DENIED on Windows CI runners")]
#[test]
fn the_env_form_overrides_the_same_way_and_names_its_variable() {
    let root = scratch("env");
    let (remote, project, home) = (root.join("remote"), root.join("project"), root.join("home"));
    init_remote(&remote);
    write_project(&project, &remote);
    let out = run_prova(&project, &home, &[], &[("PROVA_DEP_GREET", "v2")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(record_overrides(&project), vec!["greet: tag v1 -> tag v2 (PROVA_DEP_GREET)".to_string()]);
}

#[cfg_attr(windows, ignore = "local-path git fetch hits ERROR_ACCESS_DENIED on Windows CI runners")]
#[test]
fn an_undeclared_name_is_refused_with_the_declared_ones_and_the_fix() {
    let root = scratch("unknown");
    let (remote, project, home) = (root.join("remote"), root.join("project"), root.join("home"));
    init_remote(&remote);
    write_project(&project, &remote);
    let out = run_prova(&project, &home, &["--dep", "nope=v2"], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("no direct dependency named \"nope\"") && err.contains("[greet]"), "{err}");
}
