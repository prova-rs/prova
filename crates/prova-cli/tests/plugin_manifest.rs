//! End-to-end (hermetic — no docker, no network): a directory plugin with a `prova.toml [plugin]`
//! resolves via its declared `entry` even under a *different* consumer alias, vendors a sibling
//! module via the plugin namespace, and is version-gated by `requires.prova`.

use std::path::Path;
use std::process::Command;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn run(project: &Path, home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(project)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .expect("run prova")
}

/// A plugin repo whose entry is `impl.lua` (declared in prova.toml [plugin]) and which `require`s a
/// vendored sibling — pulled under the alias `greet`. Filename-matching would look for `greet.lua`
/// and fail; the manifest entry + namespace make it resolve regardless of the alias.
#[test]
fn manifest_entry_and_vendored_sibling_resolve_under_alias() {
    let root = std::env::temp_dir().join(format!("prova-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let plugin = root.join("plugin");
    let project = root.join("project");
    let home = root.join("home");

    // The plugin: entry impl.lua (NOT named after the consumer alias) + a vendored sibling words.lua.
    // A permissive `>=0.1` compat so the test tracks the workspace version across 0.x bumps.
    write(
        &plugin.join("prova.toml"),
        "[plugin]\nname = \"greeter\"\nentry = \"impl.lua\"\n\n[requires]\nprova = \">=0.1\"\n",
    );
    write(
        &plugin.join("impl.lua"),
        "local words = require(\"greeter.words\")\n\
         return { hello = function(n) return words.greeting() .. \" \" .. n end }\n",
    );
    write(
        &plugin.join("words.lua"),
        "return { greeting = function() return \"hello\" end }\n",
    );

    // The project pulls it under a DIFFERENT alias `greet` and requires that.
    write(
        &project.join("prova.toml"),
        &format!(
            "[run]\npaths = [\"tests\"]\n\n[plugins]\ngreet = {{ path = \"{}\" }}\n",
            plugin.to_string_lossy().replace('\\', "/")
        ),
    );
    write(
        &project.join("tests/greet_test.lua"),
        "local greet = require(\"greet\")\n\
         prova.test(\"manifest entry + vendored sibling resolve\", function(t)\n\
         \x20 t:expect(greet.hello(\"prova\")):equals(\"hello prova\")\n\
         end)\n",
    );

    let output = run(&project, &home);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("manifest entry + vendored sibling resolve"),
        "stdout:\n{stdout}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// A plugin declaring an impossible `requires.prova` is rejected before the run, with a clear message.
#[test]
fn incompatible_plugin_version_is_rejected() {
    let root = std::env::temp_dir().join(format!("prova-manifest-incompat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let plugin = root.join("plugin");
    let project = root.join("project");
    let home = root.join("home");

    write(
        &plugin.join("prova.toml"),
        "[plugin]\nname = \"future\"\nentry = \"impl.lua\"\n\n[requires]\nprova = \">=99.0\"\n",
    );
    write(
        &plugin.join("impl.lua"),
        "return { hello = function() return \"hi\" end }\n",
    );
    write(
        &project.join("prova.toml"),
        &format!(
            "[run]\npaths = [\"tests\"]\n\n[plugins]\nfuture = {{ path = \"{}\" }}\n",
            plugin.to_string_lossy().replace('\\', "/")
        ),
    );
    write(
        &project.join("tests/x_test.lua"),
        "prova.test(\"never runs\", function(t) t:expect(1):equals(1) end)\n",
    );

    let output = run(&project, &home);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "should fail on incompatible plugin"
    );
    assert!(stderr.contains("requires prova"), "stderr:\n{stderr}");

    std::fs::remove_dir_all(&root).ok();
}
