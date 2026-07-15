//! End-to-end: a `[sources]` alias + an `alias:repo` shorthand resolve and fetch through the real
//! `prova` binary. Proves the whole wiring — manifest `[sources]` → shorthand classification → alias
//! expansion → git fetch into the cache → `require`. A local base dir stands in for a host, so no
//! network is needed.

use std::path::Path;
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

#[test]
fn registered_alias_shorthand_is_fetched_and_required() {
    let root = std::env::temp_dir().join(format!("prova-shorthand-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let remotes = root.join("remotes");
    let repo = remotes.join("greetplugin");
    let project = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(project.join("tests")).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    // The "remote" repo, reachable by local path (stands in for github.com/acme).
    std::fs::write(
        repo.join("greet.lua"),
        "local greet = {}\nfunction greet.hello(n) return \"yo \" .. n end\nreturn greet\n",
    )
    .unwrap();
    git(&["init", "-q"], &repo);
    git(&["add", "."], &repo);
    git(&["commit", "-q", "-m", "greet"], &repo);

    // Manifest: register the local `remotes` dir as source alias `acme`, then use `acme:greetplugin`.
    std::fs::write(
        project.join("prova.toml"),
        format!(
            "[run]\npaths = [\"tests\"]\n\n[sources]\nacme = \"{}\"\n\n[plugins]\ngreet = \"acme:greetplugin\"\n",
            remotes.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();
    std::fs::write(
        project.join("tests").join("greet_test.lua"),
        "local greet = require(\"greet\")\n\
         prova.test(\"shorthand plugin resolves\", function(t)\n\
         \x20 t:expect(greet.hello(\"prova\")):equals(\"yo prova\")\n\
         end)\n",
    )
    .unwrap();

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
    assert!(
        stdout.contains("shorthand plugin resolves"),
        "stdout:\n{stdout}"
    );

    std::fs::remove_dir_all(&root).ok();
}
