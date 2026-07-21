//! End-to-end: a `[plugins]` git source is fetched into the cache and resolved by `require`, driven
//! through the real `prova` binary. Proves the whole path — manifest parse → git clone/checkout into
//! the XDG cache → searcher resolves the named plugin → the test that `require`s it passes.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str], cwd: &Path) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        // Deterministic identity so `commit` works on a bare CI runner.
        .env("GIT_AUTHOR_NAME", "prova")
        .env("GIT_AUTHOR_EMAIL", "prova@example.com")
        .env("GIT_COMMITTER_NAME", "prova")
        .env("GIT_COMMITTER_EMAIL", "prova@example.com")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn manifest_git_plugin_is_fetched_and_required() {
    // A unique scratch root (no Date/rand in the harness — use the test binary's pid).
    let root = std::env::temp_dir().join(format!("prova-git-plugin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let remote = root.join("remote");
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(project.join("tests")).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    // Build the "remote" plugin repo: one namespace module, committed.
    std::fs::write(
        remote.join("greet.lua"),
        "local greet = {}\nfunction greet.hello(n) return \"hi \" .. n end\nreturn greet\n",
    )
    .unwrap();
    git(&["init", "-q"], &remote);
    git(&["add", "."], &remote);
    git(&["commit", "-q", "-m", "greet plugin"], &remote);
    let rev = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&remote)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // The project: a manifest declaring the git plugin (pinned to the rev) and a test that requires it.
    std::fs::write(
        project.join("prova.toml"),
        format!(
            "[run]\nproofs = [\"tests\"]\n\n[plugins]\ngreet = {{ git = \"{}\", rev = \"{}\" }}\n",
            remote.to_string_lossy().replace('\\', "/"),
            rev
        ),
    )
    .unwrap();
    std::fs::write(
        project.join("tests").join("greet_test.lua"),
        "local greet = require(\"greet\")\n\
         prova.test(\"git plugin resolves\", function(t)\n\
         \x20 t:expect(greet.hello(\"prova\")):equals(\"hi prova\")\n\
         end)\n",
    )
    .unwrap();

    // Run the real binary with an isolated XDG cache so the checkout lands under our temp home.
    let output = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&project)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .expect("run prova");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("git plugin resolves"), "stdout:\n{stdout}");

    // The plugin was fetched into the XDG cache, pinned by rev.
    assert!(
        home.join("cache").join("prova").join("plugins").exists(),
        "expected a git-plugin checkout under the cache"
    );

    std::fs::remove_dir_all(&root).ok();
}
