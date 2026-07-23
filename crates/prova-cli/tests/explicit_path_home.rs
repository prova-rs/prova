//! End-to-end (hermetic — no docker, no network): explicit path arguments select WHAT to run but
//! must not strip the package environment. `prova tests/x_test.lua` discovers the prova home from
//! the named path (not just the cwd), resolves the manifest's `[plugins]` so `require(...)` works
//! exactly as in a manifest run, and keeps a named file's suite membership (a sibling `suite.lua`
//! still wraps it). Paths outside any package still run bare, and paths spanning two packages are
//! refused rather than guessed at.

use std::path::Path;
use std::process::Command;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn prova(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .args(args)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .expect("run prova")
}

/// A scratch tree, removed on drop even when an assertion fails mid-test.
struct Tmp(std::path::PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("prova-explicit-{tag}-{}", std::process::id()));
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

/// A minimal package: a path plugin under `[plugins]` and one test file that `require`s it.
fn plugin_package(root: &Path) {
    write(
        &root.join("plugin/prova.toml"),
        "[plugin]\nname = \"greeter\"\nentry = \"impl.lua\"\n\n[requires]\nprova = \">=0.1\"\n",
    );
    write(
        &root.join("plugin/impl.lua"),
        "return { hello = function(n) return \"hello \" .. n end }\n",
    );
    write(
        &root.join("project/prova.toml"),
        "[run]\nproofs = [\"tests\"]\n\n[plugins]\ngreet = { path = \"../plugin\" }\n",
    );
    write(
        &root.join("project/tests/greet_test.lua"),
        "local greet = require(\"greet\")\n\
         prova.test(\"explicit file resolves the manifest plugin\", function(t)\n\
         \x20 t:expect(greet.hello(\"prova\")):equals(\"hello prova\")\n\
         end)\n",
    );
}

// The regression that motivated this: from the package root, naming the file must resolve the
// package's `[plugins]` exactly as the bare manifest run does.
#[test]
fn explicit_file_resolves_manifest_plugins() {
    let t = Tmp::new("plugins");
    plugin_package(&t.0);
    let output = prova(&t.0.join("project"), &t.0, &["tests/greet_test.lua"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("explicit file resolves the manifest plugin"),
        "stdout:\n{stdout}"
    );
}

// The home anchors at the NAMED PATH, not the cwd: running from a neutral directory (no manifest
// anywhere above it) against a file inside a package must still find that package's plugins.
#[test]
fn explicit_file_anchors_home_at_the_file_not_the_cwd() {
    let t = Tmp::new("anchor");
    plugin_package(&t.0);
    let neutral = t.0.join("elsewhere");
    std::fs::create_dir_all(&neutral).unwrap();
    let file = t.0.join("project/tests/greet_test.lua");
    let output = prova(&neutral, &t.0, &[file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("explicit file resolves the manifest plugin"),
        "stdout:\n{stdout}"
    );
}

// Selection narrows the files, never their environment: a named file whose directory carries a
// `suite.lua` still runs UNDER that suite (its setup runs, its Scope.Suite fixtures resolve).
#[test]
fn explicit_file_keeps_sibling_suite_membership() {
    let t = Tmp::new("suite");
    write(&t.0.join("project/prova.toml"), "[run]\nproofs = [\"proofs\"]\n");
    write(
        &t.0.join("project/proofs/suite.lua"),
        "suite.config{ name = \"orders\" }\n\
         prova.fixture(\"store\", Scope.Suite, function(ctx)\n\
         \x20 return { tag = \"from-suite-setup\" }\n\
         end)\n",
    );
    write(
        &t.0.join("project/proofs/read_test.lua"),
        "prova.test(\"sees the suite fixture\", function(t)\n\
         \x20 t:expect(t:use(\"store\").tag):equals(\"from-suite-setup\")\n\
         end)\n",
    );
    let output = prova(&t.0.join("project"), &t.0, &["proofs/read_test.lua"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("sees the suite fixture"), "stdout:\n{stdout}");
}

// A file outside any package keeps working exactly as before: built-ins only, no manifest needed.
#[test]
fn explicit_file_outside_any_package_still_runs_bare() {
    let t = Tmp::new("bare");
    write(
        &t.0.join("loose/adhoc_test.lua"),
        "prova.test(\"runs with builtins only\", function(t)\n\
         \x20 t:expect(1 + 1):equals(2)\n\
         end)\n",
    );
    let output = prova(&t.0.join("loose"), &t.0, &["adhoc_test.lua"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("runs with builtins only"), "stdout:\n{stdout}");
}

// Explicit paths spanning TWO packages would need two environments in one run — refuse loudly
// instead of running half the files with the wrong plugins.
#[test]
fn explicit_files_spanning_two_packages_are_refused() {
    let t = Tmp::new("span");
    for pkg in ["one", "two"] {
        write(
            &t.0.join(pkg).join("prova.toml"),
            "[run]\nproofs = [\"proofs\"]\n",
        );
        write(
            &t.0.join(pkg).join("proofs/x_test.lua"),
            "prova.test(\"x\", function(t) t:expect(1):equals(1) end)\n",
        );
    }
    let output = prova(
        &t.0,
        &t.0,
        &["one/proofs/x_test.lua", "two/proofs/x_test.lua"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "expected refusal.\nstderr:\n{stderr}");
    assert!(
        stderr.contains("packages") || stderr.contains("package"),
        "stderr should name the multi-package conflict:\n{stderr}"
    );
}

// A manifest with `[plugins]` but NO `[run] proofs` cannot run bare — but an explicit file names
// the selection itself, so it must not be blocked by the missing key.
#[test]
fn explicit_file_runs_in_a_package_that_declares_no_proofs() {
    let t = Tmp::new("noproofs");
    write(
        &t.0.join("plugin/prova.toml"),
        "[plugin]\nname = \"greeter\"\nentry = \"impl.lua\"\n\n[requires]\nprova = \">=0.1\"\n",
    );
    write(
        &t.0.join("plugin/impl.lua"),
        "return { hello = function(n) return \"hello \" .. n end }\n",
    );
    write(
        &t.0.join("project/prova.toml"),
        "[plugins]\ngreet = { path = \"../plugin\" }\n",
    );
    write(
        &t.0.join("project/scratch/greet_test.lua"),
        "local greet = require(\"greet\")\n\
         prova.test(\"proofs key not required for explicit runs\", function(t)\n\
         \x20 t:expect(greet.hello(\"x\")):equals(\"hello x\")\n\
         end)\n",
    );
    let output = prova(&t.0.join("project"), &t.0, &["scratch/greet_test.lua"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("proofs key not required for explicit runs"),
        "stdout:\n{stdout}"
    );
}
