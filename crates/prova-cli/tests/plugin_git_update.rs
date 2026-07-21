//! End-to-end proofs that a `[plugins]` git source honors the shared two-gate freshness check,
//! driven through the real `prova` binary against a local git remote.
//!
//! The observable signals are (a) prova's exit status — a plugin whose returned value changed makes
//! the project's test flip pass↔fail — and (b) the update messages prova prints on stderr:
//! `fetching plugin` on first clone, `updating plugin` when a fetch actually happens, and **silence**
//! when the cache is confirmed current. That silence is the crux of the feature: a repo already
//! matching its remote produces no pull message.

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

/// A remote plugin repo whose `greet.tag()` returns `tag`, committed on `main`.
fn init_plugin_remote(remote: &Path, tag: &str) {
    std::fs::create_dir_all(remote).unwrap();
    write_plugin(remote, tag);
    git(&["init", "-q", "-b", "main"], remote);
    git(&["add", "."], remote);
    git(&["commit", "-q", "-m", "v1"], remote);
}

fn write_plugin(remote: &Path, tag: &str) {
    std::fs::write(
        remote.join("greet.lua"),
        format!("local greet = {{}}\nfunction greet.tag() return \"{tag}\" end\nreturn greet\n"),
    )
    .unwrap();
}

/// Change the plugin's tag on the remote and commit it.
fn move_plugin_remote(remote: &Path, tag: &str) {
    write_plugin(remote, tag);
    git(&["add", "."], remote);
    git(&["commit", "-q", "-m", "moved"], remote);
}

/// Write the consumer project: a manifest declaring the git plugin (default branch → mutable, so the
/// hash gate applies) and a test asserting the plugin's tag equals `expect`.
fn write_project(project: &Path, remote: &Path, expect: &str, updates_toml: &str) {
    std::fs::create_dir_all(project.join("tests")).unwrap();
    std::fs::write(
        project.join("prova.toml"),
        format!(
            "[run]\nproofs = [\"tests\"]\n\n[plugins]\ngreet = {{ git = \"{}\" }}\n{updates_toml}",
            remote.to_string_lossy().replace('\\', "/"),
        ),
    )
    .unwrap();
    std::fs::write(
        project.join("tests").join("greet_test.lua"),
        format!(
            "local greet = require(\"greet\")\n\
             prova.test(\"tag\", function(t)\n\
             \x20 t:expect(greet.tag()):equals(\"{expect}\")\n\
             end)\n",
        ),
    )
    .unwrap();
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("prova-git-update-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Run the real `prova` binary in `project`, with an isolated XDG cache under `home`.
fn run_prova(project: &Path, home: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(project)
        .args(extra)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .expect("run prova")
}

/// The TTL gate: within the interval a cached branch plugin is used as-is, even after the remote
/// moves, with no network and no message — and `--update` forces the refresh.
#[test]
fn ttl_gate_holds_stale_until_forced_update() {
    let root = scratch("ttl");
    let remote = root.join("remote");
    let project = root.join("project");
    let home = root.join("home");
    init_plugin_remote(&remote, "one");
    // Default interval (7 days) — the TTL never lapses during the test.
    write_project(&project, &remote, "one", "");

    // Phase A: first run clones the plugin and passes.
    let a = run_prova(&project, &home, &[]);
    let a_err = String::from_utf8_lossy(&a.stderr);
    assert!(a.status.success(), "first run should pass\n{a_err}");
    assert!(
        a_err.contains("fetching plugin"),
        "expected a clone message\n{a_err}"
    );

    // The remote moves to "two" — but the cache still holds "one".
    move_plugin_remote(&remote, "two");

    // Phase B: a second run within the TTL uses the cache: still "one", still passing, and SILENT —
    // no fetch, no "updating" message, because the TTL gate short-circuits before any network.
    let b = run_prova(&project, &home, &[]);
    let b_err = String::from_utf8_lossy(&b.stderr);
    assert!(
        b.status.success(),
        "stale run should still pass on cached 'one'\n{b_err}"
    );
    assert!(
        !b_err.contains("updating plugin"),
        "TTL-fresh run must be silent\n{b_err}"
    );
    assert!(
        !b_err.contains("fetching plugin"),
        "TTL-fresh run must not re-fetch\n{b_err}"
    );

    // Phase C: `--update` skips the gate, fetches "two"; the test now fails (asserts "one") and prova
    // announces the update.
    let c = run_prova(&project, &home, &["--update"]);
    let c_err = String::from_utf8_lossy(&c.stderr);
    assert!(
        !c.status.success(),
        "forced update should surface the changed plugin\n{c_err}"
    );
    assert!(
        c_err.contains("updating plugin"),
        "forced update should announce it\n{c_err}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// The hash gate: with the TTL forced to expire (`interval = "0s"`), an unchanged remote is confirmed
/// current via `ls-remote` and stays SILENT; only a moved remote triggers a pull and its message.
#[test]
fn hash_gate_updates_only_when_remote_moved() {
    // Case 1: remote unchanged → silent confirmation, no update.
    {
        let root = scratch("hash-same");
        let remote = root.join("remote");
        let project = root.join("project");
        let home = root.join("home");
        init_plugin_remote(&remote, "one");
        write_project(&project, &remote, "one", "\n[updates]\ninterval = \"0s\"\n");

        let seed = run_prova(&project, &home, &[]);
        assert!(seed.status.success(), "seed run should pass");

        // TTL is 0s, so the gate probes — but the remote hasn't moved, so ls-remote matches: no
        // fetch, no message.
        let again = run_prova(&project, &home, &[]);
        let err = String::from_utf8_lossy(&again.stderr);
        assert!(
            again.status.success(),
            "unchanged remote should still pass\n{err}"
        );
        assert!(
            !err.contains("updating plugin"),
            "a matching remote hash must NOT print an update message\n{err}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // Case 2: remote moved → hash differs → pull + message + the change is seen.
    {
        let root = scratch("hash-moved");
        let remote = root.join("remote");
        let project = root.join("project");
        let home = root.join("home");
        init_plugin_remote(&remote, "one");
        write_project(&project, &remote, "one", "\n[updates]\ninterval = \"0s\"\n");

        let seed = run_prova(&project, &home, &[]);
        assert!(seed.status.success(), "seed run should pass");

        move_plugin_remote(&remote, "two");

        let moved = run_prova(&project, &home, &[]);
        let err = String::from_utf8_lossy(&moved.stderr);
        assert!(
            err.contains("updating plugin"),
            "a moved remote should pull + announce\n{err}"
        );
        assert!(
            !moved.status.success(),
            "the pulled change ('two') should flip the test\n{err}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

/// `--offline` never touches the network: a cached plugin resolves even with the remote deleted.
#[test]
fn offline_uses_cache_without_network() {
    let root = scratch("offline");
    let remote = root.join("remote");
    let project = root.join("project");
    let home = root.join("home");
    init_plugin_remote(&remote, "one");
    write_project(&project, &remote, "one", "");

    let seed = run_prova(&project, &home, &[]);
    assert!(seed.status.success(), "seed run should pass");

    // Delete the remote; only the cache remains.
    std::fs::remove_dir_all(&remote).unwrap();

    let offline = run_prova(&project, &home, &["--offline"]);
    let err = String::from_utf8_lossy(&offline.stderr);
    assert!(
        offline.status.success(),
        "offline run should resolve from cache\n{err}"
    );
    assert!(
        !err.contains("updating plugin"),
        "offline must not fetch\n{err}"
    );

    std::fs::remove_dir_all(&root).ok();
}
