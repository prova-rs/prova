use std::path::PathBuf;

use prova_core::{discover_suites, run_suites, NullReporter, RunConfig};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name)
}

/// The require-root is configurable, not hardwired to the home (layout plan, option 3).
///
/// The home is the repo root, but `shared/thing.lua` lives under `proofs/`. With the require-root
/// set to `proofs/`, `require("shared.thing")` resolves `proofs/shared/thing.lua` — NOT
/// `<home>/shared/thing.lua`, which does not exist. This is what lets `require("shared.x")` read
/// cleanly from within `proofs/`, and it is the seam the `**/proofs` multi-root discovery builds on
/// (each root resolves its own requires locally).
#[test]
fn require_resolves_against_a_configured_root() {
    let home = testdata("layout_require_roots");
    let proofs = home.join("proofs");
    let suites = discover_suites(&proofs).expect("discover");
    let mut reporter = NullReporter;
    let config = RunConfig::new(1)
        .with_project(&home, &home)
        .with_require_roots(vec![proofs.clone()]);
    let summary = run_suites(&suites, &mut reporter, &config).expect("run");
    assert_eq!(summary.failed, 0, "failed");
    assert_eq!(summary.passed, 1, "require rooted at the configured dir resolves");
}
