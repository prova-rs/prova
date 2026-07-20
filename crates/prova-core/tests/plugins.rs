use std::path::PathBuf;

use prova_core::{run_path_with, NullReporter, RunConfig};

/// The plugin searcher resolves both a **bundled** first-party module (`prova.workspace`) and a
/// **disk** plugin (`greet`), and reports a clean error on a miss.
///
/// The disk root is passed explicitly rather than injected through `PROVA_PLUGIN_PATH`. That env var
/// is gone: a root reachable from the environment is a root you cannot read off the project, which
/// is the same "works on my machine" hole as a per-user plugin directory. An embedder — a test very
/// much included — names its root, exactly as the CLI now passes the manifest's `[run] plugin_root`.
#[test]
fn require_resolves_bundled_and_disk_plugins() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest.join("testdata").join("plugin_require.lua");
    let config = RunConfig::new(1).with_plugin_root(manifest.join("testdata").join("plugins"));

    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &config).expect("run plugin_require.lua");

    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 3, "passed");
}
