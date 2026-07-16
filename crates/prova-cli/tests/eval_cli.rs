//! End-to-end for `prova eval` through the real binary: expression and statement snippets, the
//! built-in globals, error exit codes, and (docker-gated) a manifest plugin provisioning a real
//! container through the transient `ctx` — with teardown verified against the docker daemon.

use std::path::Path;
use std::process::{Command, Stdio};

fn eval_in(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(dir)
        .arg("eval")
        .args(args)
        .output()
        .expect("run prova eval")
}

/// A scratch dir with NO prova.toml anywhere above it, so eval runs manifest-less.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-eval-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn expression_prints_its_value_and_exits_zero() {
    let dir = scratch("expr");
    let out = eval_in(&dir, &["return 1 + 1"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn statements_with_an_explicit_return_work() {
    let dir = scratch("stmt");
    let out = eval_in(
        &dir,
        &["local t = {}\nfor i = 1, 3 do t[i] = i * i end\nreturn t[3]"],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "9");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_fs_global_is_installed() {
    let dir = scratch("fs");
    std::fs::write(dir.join("probe.txt"), "x").unwrap();
    let out = eval_in(&dir, &["return fs.exists('probe.txt')"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_table_pretty_prints_as_json_and_nil_prints_nothing() {
    let dir = scratch("json");
    let out = eval_in(&dir, &["return { name = 'prova', n = 2 }"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(v["name"], "prova");
    assert_eq!(v["n"], 2);

    let out = eval_in(&dir, &["return nil"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");

    // --format json forces JSON even for scalars.
    let out = eval_in(&dir, &["--format", "json", "return 'hi'"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "\"hi\"");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_raising_snippet_exits_one_with_the_error_on_stderr() {
    let dir = scratch("err");
    let out = eval_in(&dir, &["error('boom')"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("boom"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn no_snippet_is_a_usage_error() {
    let dir = scratch("usage");
    let out = eval_in(&dir, &[]);
    assert_eq!(out.status.code(), Some(2));
    std::fs::remove_dir_all(&dir).ok();
}

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The whole point of `prova eval`: `require` a manifest-declared plugin, provision a real
/// container through the transient `ctx`, print its endpoint — and have the container reaped by
/// scope teardown before the process exits. Skips (like every docker test) when docker is absent.
#[test]
fn manifest_plugin_provisions_and_ctx_tears_down() {
    if !docker_available() {
        eprintln!("skipping: docker is not available");
        return;
    }
    let dir = scratch("docker");
    // A local single-file plugin wrapping the built-in docker module, declared in the manifest —
    // the same resolution path a git plugin (e.g. postgres) takes.
    std::fs::write(
        dir.join("whoami.lua"),
        "local whoami = {}\n\
         function whoami.container(ctx)\n\
         \x20 local c = docker.run{ image = 'traefik/whoami', ports = { 80 },\n\
         \x20\x20\x20\x20 wait = { port = 80, timeout = '60s' } }\n\
         \x20 ctx:defer(function() c:stop() end)\n\
         \x20 return { url = 'http://' .. c:endpoint(80), id = c.id }\n\
         end\n\
         return whoami\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("prova.toml"),
        "[run]\npaths = [\".\"]\n\n[luals]\nmanage = \"never\"\n\n[plugins]\nwhoami = { path = \"whoami.lua\" }\n",
    )
    .unwrap();

    let out = eval_in(
        &dir,
        &[
            "--format",
            "json",
            "local svc = require('whoami').container(ctx); return svc",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert!(
        v["url"].as_str().is_some_and(|u| u.starts_with("http://")),
        "endpoint printed: {v}"
    );
    let id = v["id"].as_str().expect("container id in the result");

    // Teardown ran before exit: the daemon no longer knows a running container with that id.
    let ps = Command::new("docker")
        .args(["ps", "--no-trunc", "--format", "{{.ID}}"])
        .output()
        .expect("docker ps");
    let running = String::from_utf8_lossy(&ps.stdout);
    assert!(
        !running.contains(id),
        "container {id} was not reaped by ctx teardown; running:\n{running}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
