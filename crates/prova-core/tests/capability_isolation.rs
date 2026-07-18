use std::path::PathBuf;

use prova_core::{load_project_config, RunConfig};

/// Two project companions, loaded in ONE process — exactly what the warm MCP does when it resolves
/// project A at startup and then `run { project = "B" }`. Each project's capabilities must be its
/// own: B must never inherit what only A's `prova.lua` registered.
///
/// This is the bug the first `prova.capability` cut introduced: a process-global registry, populated
/// per resolve and never cleared, so the second project saw the first's capabilities. The fix makes
/// registration a per-load value ([`prova_core::Capabilities`]) carried in `RunConfig`, so there is
/// no shared state to leak through.
fn companion(dir: &PathBuf, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn companions_do_not_leak_across_resolves() {
    let cfg = RunConfig::new(1);
    let dir = std::env::temp_dir().join(format!("prova-caps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let a = companion(
        &dir,
        "a.lua",
        r#"runtime.capability("iso_a", function() return true end)"#,
    );
    let b = companion(
        &dir,
        "b.lua",
        r#"runtime.capability("iso_b", function() return true end)"#,
    );

    // Each resolve returns its OWN capability set. Same process, back to back.
    let caps_a = load_project_config(&a, &cfg).expect("load A");
    let caps_b = load_project_config(&b, &cfg).expect("load B");

    // A saw its own, B saw its own — the sanity legs.
    assert!(
        caps_a.available("iso_a"),
        "A's own capability present in A's set"
    );
    assert!(
        caps_b.available("iso_b"),
        "B's own capability present in B's set"
    );

    // THE PROOF: B's set must not contain A's capability. B is a different project; it cannot inherit
    // A's vocabulary. With the process-global registry it did — this is the isolation the fix buys.
    assert!(
        !caps_b.available("iso_a"),
        "project B inherited project A's capability — capabilities are not per-resolve"
    );

    // …and built-ins still work through any set (registered names are consulted first, then these).
    assert!(caps_a.available("unix") == cfg!(unix));

    std::fs::remove_dir_all(&dir).ok();
}
