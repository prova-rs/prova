use std::path::PathBuf;

use prova_core::{discover_suites, run_suites, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// The bundled + isolated plugin-composition contract: a plugin declares private dependencies in its
/// own `prova.toml` (`[plugins]`), those names resolve for *that plugin's* code, and for
/// nobody else.
///
/// The fixture makes the isolation load-bearing rather than incidental: `alpha` and `beta` privately
/// depend on two *different* plugins that both answer to `store`. A single shared namespace could not
/// satisfy that at all — one would shadow the other — so the proof fails loudly if resolution ever
/// stops being per-plugin.
///
/// Three separate leak paths are covered, because closing only the obvious one is not enough:
/// the searcher must not resolve `store` globally; `package.loaded` (keyed by *name*) must not cache
/// it; and — the one that actually bit during implementation — installing the scoped `require` on a
/// plugin environment must be a RAW set, or `__newindex` forwards it to the real globals and every
/// consumer inherits the plugin's private map.
#[test]
fn private_deps_resolve_per_plugin_and_stay_private() {
    let root = testdata("plugin_private_deps");
    let suites = discover_suites(&root.join("proofs")).expect("discover");
    let mut reporter = NullReporter;
    let config = RunConfig::new(1)
        .with_project(&root)
        .with_plugin_root(root.join(".prova/plugins"));
    let summary = run_suites(&suites, &mut reporter, &config).expect("run");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 3, "passed");
}
