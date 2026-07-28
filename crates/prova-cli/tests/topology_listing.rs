//! `prova up` with no topology name lists what's defined — the discovery half of the `up` verb,
//! mirroring how `prova init` lists templates. Listing only *registers* topologies (execs the files);
//! it never invokes a factory, so it needs no docker.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-uplist-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn up_no_arg(cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .arg("up")
        .output()
        .unwrap();
    let combined =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), combined)
}

/// With topologies registered, `prova up` (no arg) lists their names.
///
/// Registered, not declared in code: the inhabited verbs resolve `[topologies]` only, so listing
/// reports the environments a project actually publishes rather than every fixture a test happens
/// to build in-process.
#[test]
fn up_with_no_arg_lists_defined_topologies() {
    let dir = tmp("some");
    write(
        &dir,
        ".prova.toml",
        "[run]\nproofs = [\".\"]\n\
         [plugins]\nfixture = { path = \"./fixture\" }\n\
         [topologies]\n\
         web = { plugin = \"fixture\", factory = \"web\" }\n\
         db = { plugin = \"fixture\", factory = \"db\" }\n",
    );
    write(&dir, "fixture/prova.toml", "[plugin]\nname = \"fixture\"\n");
    write(
        &dir,
        "fixture/init.lua",
        "local M = {}\n\
         function M.web(ctx) return { url = \"http://x\" } end\n\
         function M.db(ctx) return {} end\n\
         return M\n",
    );
    let (ok, out) = up_no_arg(&dir);
    assert!(ok, "listing should succeed: {out}");
    assert!(out.contains("web"), "lists `web`: {out}");
    assert!(out.contains("db"), "lists `db`: {out}");
}

/// With none defined, it says so — an actionable message, not an empty success or a usage dump.
#[test]
fn up_with_no_arg_and_no_topologies_says_none() {
    let dir = tmp("none");
    write(&dir, ".prova.toml", "[run]\nproofs = [\".\"]\n");
    write(
        &dir,
        "plain_test.lua",
        "prova.test(\"x\", function(t) t:expect(1):equals(1) end)\n",
    );
    let (_ok, out) = up_no_arg(&dir);
    assert!(
        out.to_lowercase().contains("no topolog"),
        "says none are defined: {out}"
    );
}
