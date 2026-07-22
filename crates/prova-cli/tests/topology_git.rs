//! The git forms of `prova up`: point at a repo that advertises topologies. `prova up <url>` lists
//! what it offers; `prova up <topology> <url>` stands that one up. The repo is fetched (pinned) the
//! same way a git `[plugins]` source is. Hermetic: a local git repo served over a `file://` URL.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Build a "remote" git repo that IS a topology plugin: it advertises `linux-vm`. Returns
/// `(scratch_root, file_url)`.
fn remote_topology_plugin(tag: &str, requires: &str) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!("prova-topogit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let remote = root.join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::write(
        remote.join("prova.toml"),
        format!(
            "[plugin]\nname = \"parallels\"\n\n\
             [[plugin.topologies]]\nname = \"linux-vm\"\nfactory = \"linux_vm\"\n{requires}"
        ),
    )
    .unwrap();
    std::fs::write(
        remote.join("init.lua"),
        "return { linux_vm = function(ctx) return { url = \"vm\" } end }\n",
    )
    .unwrap();
    git(&["init", "-q"], &remote);
    git(&["add", "."], &remote);
    git(&["commit", "-q", "-m", "topology plugin"], &remote);
    let url = format!("file://{}", remote.to_string_lossy().replace('\\', "/"));
    (root, url)
}

/// Run `prova up <args>` from a neutral cwd with an isolated XDG cache (for the fetch). Returns
/// (success, combined output). A watchdog kills a held `up` so a stand-up can't hang the test.
fn up(args: &[&str], xdg: &Path) -> (bool, String) {
    use std::io::Read;
    use std::time::{Duration, Instant};
    let mut child = Command::new(env!("CARGO_BIN_EXE_prova"))
        .arg("up")
        .args(args)
        .current_dir(std::env::temp_dir())
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            break None;
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

/// `prova up <url>` fetches the repo and lists the topologies it advertises. RED today: `up`'s single
/// argument is read as a local topology NAME, so a URL just isn't found.
#[cfg_attr(windows, ignore = "local-path git fetch hits ERROR_ACCESS_DENIED on Windows CI runners")]
#[test]
fn up_url_lists_the_repos_advertised_topologies() {
    let (root, url) = remote_topology_plugin("list", "");
    let xdg = root.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let (ok, out) = up(&[&url], &xdg);
    std::fs::remove_dir_all(&root).ok();
    assert!(ok, "listing a repo's topologies should succeed: {out}");
    assert!(
        out.contains("linux-vm"),
        "lists the advertised topology: {out}"
    );
}

/// `prova up <topology> <url>` where the repo doesn't advertise that name fails loudly, listing what
/// it does advertise — the fetch happened and the advertisement was consulted.
#[cfg_attr(windows, ignore = "local-path git fetch hits ERROR_ACCESS_DENIED on Windows CI runners")]
#[test]
fn up_topology_url_rejects_an_unadvertised_name() {
    let (root, url) = remote_topology_plugin("badname", "");
    let xdg = root.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let (ok, out) = up(&["windows-vm", &url], &xdg);
    std::fs::remove_dir_all(&root).ok();
    assert!(!ok, "an unadvertised name must fail: {out}");
    assert!(
        out.contains("linux-vm"),
        "lists what the repo advertises: {out}"
    );
}

/// `prova up <topology> <url>` honors the topology's advertised `requires`: an unmet one blocks the
/// stand-up (after the fetch), before provisioning.
#[cfg_attr(windows, ignore = "local-path git fetch hits ERROR_ACCESS_DENIED on Windows CI runners")]
#[test]
fn up_topology_url_honors_advertised_requires() {
    let (root, url) = remote_topology_plugin("req", "requires = [\"definitelymissing\"]\n");
    let xdg = root.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let (ok, out) = up(&["linux-vm", &url], &xdg);
    std::fs::remove_dir_all(&root).ok();
    assert!(!ok, "an unmet requirement must block up: {out}");
    assert!(
        out.contains("definitelymissing"),
        "names the unmet requirement: {out}"
    );
}
