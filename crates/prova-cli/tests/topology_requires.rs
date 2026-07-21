//! A topology declares the environment it needs (`requires`), and `prova up` honors it: an unmet
//! requirement is caught before anything is provisioned, with a reason that names it. Requirements
//! come from the plugin's advertisement (the topology's own contract) and/or the project's
//! registration (a local addition), and merge.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-toporeq-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Run `prova up <name>` with a watchdog. The requirement gate exits immediately; a topology that
/// actually stands up would *hold* until Ctrl-C, so if it hasn't exited by the deadline the gate did
/// not fire — kill it and report that, rather than hanging the test.
fn up(cwd: &Path, name: &str) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .args(["up", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            break None; // held past the deadline → no gate
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        s.read_to_string(&mut out).ok();
    }
    if let Some(mut s) = child.stderr.take() {
        s.read_to_string(&mut out).ok();
    }
    (status.map(|s| s.success()).unwrap_or(false), out)
}

// `definitelymissing` is never a registered capability nor a host built-in, so its requirement is
// always unmet — a deterministic, hermetic gate.
const MISSING: &str = "definitelymissing";

/// A `[topologies]` registration can declare `requires`; an unmet one blocks `up` before provisioning.
/// RED today: `requires` is unknown to the topology declaration.
#[test]
fn a_registration_requirement_gates_up() {
    let dir = tmp("reg");
    write(
        &dir,
        ".prova/plugins/site/init.lua",
        "return { web = function(ctx) return {} end }\n",
    );
    write(
        &dir,
        "proofs/p_test.lua",
        "prova.test(\"p\", function(t) t:expect(1):equals(1) end)\n",
    );
    write(
        &dir,
        ".prova.toml",
        &format!(
            "[run]\nproofs = [\"proofs\"]\nplugin_root = \".prova/plugins\"\n\n\
             [topologies]\nvm = {{ plugin = \"site\", factory = \"web\", requires = [\"{MISSING}\"] }}\n"
        ),
    );
    let (ok, out) = up(&dir, "vm");
    assert!(!ok, "an unmet requirement must block up: {out}");
    assert!(out.contains(MISSING), "names the unmet requirement: {out}");
}

/// The plugin's advertisement carries the topology's own requirements, and they gate `up` for any
/// project that registers it — the requirement travels with the topology, not the call site.
#[test]
fn an_advertised_requirement_gates_up() {
    let dir = tmp("adv");
    write(
        &dir,
        "pg/prova.toml",
        &format!(
            "[plugin]\nname = \"pg\"\n\n\
             [[plugin.topologies]]\nname = \"single\"\nfactory = \"single\"\nrequires = [\"{MISSING}\"]\n"
        ),
    );
    write(
        &dir,
        "pg/init.lua",
        "return { single = function(ctx) return {} end }\n",
    );
    write(
        &dir,
        "proofs/p_test.lua",
        "prova.test(\"p\", function(t) t:expect(1):equals(1) end)\n",
    );
    write(
        &dir,
        ".prova.toml",
        "[run]\nproofs = [\"proofs\"]\n\n[plugins]\npg = { path = \"pg\" }\n\n\
         [topologies]\ndb = { plugin = \"pg\", topology = \"single\" }\n",
    );
    let (ok, out) = up(&dir, "db");
    assert!(!ok, "an advertised requirement must block up: {out}");
    assert!(out.contains(MISSING), "names the unmet requirement: {out}");
}
