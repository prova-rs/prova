use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// The plugin searcher resolves both a **bundled** first-party module (`prova.workspace`) and a
/// **disk** plugin (`greet`, from `PROVA_PLUGIN_PATH`), and reports a clean error on a miss.
#[test]
fn require_resolves_bundled_and_disk_plugins() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Point the searcher at the on-disk example plugin.
    std::env::set_var("PROVA_PLUGIN_PATH", manifest.join("testdata").join("plugins"));

    let path = manifest.join("testdata").join("plugin_require.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run plugin_require.lua");

    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
