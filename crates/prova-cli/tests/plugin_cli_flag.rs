//! End-to-end: an ad-hoc `--plugin name=source` (no manifest) is resolved and `require`-able,
//! through the real `prova` binary. This is what the GitHub Action's `plugins:` input expands to.

use std::process::Command;

#[test]
fn cli_plugin_flag_is_resolved_and_required() {
    let root = std::env::temp_dir().join(format!("prova-cli-plugin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("tests")).unwrap();

    std::fs::write(
        root.join("greet.lua"),
        "local greet = {}\nfunction greet.hi() return \"hi\" end\nreturn greet\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests").join("greet_test.lua"),
        "local greet = require(\"greet\")\n\
         prova.test(\"cli plugin resolves\", function(t)\n\
         \x20 t:expect(greet.hi()):equals(\"hi\")\n\
         end)\n",
    )
    .unwrap();

    // Explicit path bypasses the manifest; the plugin comes only from --plugin.
    let output = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&root)
        .args(["--plugin", "greet=./greet.lua", "tests/greet_test.lua"])
        .output()
        .expect("run prova");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "prova failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("cli plugin resolves"), "stdout:\n{stdout}");

    std::fs::remove_dir_all(&root).ok();
}
