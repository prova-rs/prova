//! End-to-end for the detached topology supervisor: `prova start` → `prova ps` → `prova down`,
//! through the real binary, with a no-docker topology (fast, CI-safe). Proves the whole design:
//! `start` spawns a detached `prova up` that self-registers, `ps` lists it, and `down` signals it so
//! the *detached child* runs the same in-process teardown (verified by a marker file it writes).
//!
//! Unix-only: `down` tears down via SIGTERM, which the held `prova up` handles.
#![cfg(unix)]

use std::process::Command;

fn prova(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(root)
        .args(args)
        .output()
        .expect("run prova")
}

#[test]
fn start_ps_down_supervises_a_detached_topology() {
    let root = std::env::temp_dir().join(format!("prova-topo-life-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let marker = root.join("teardown.marker");

    std::fs::write(
        root.join("prova.toml"),
        "[run]\npaths = [\".\"]\n[luals]\nmanage = \"never\"\n",
    )
    .unwrap();
    // A no-docker topology: two fake resources with `url`s, and a deferred teardown that appends to a
    // marker file — so we can prove the detached child ran teardown when `down` signalled it.
    std::fs::write(
        root.join("orders_test.lua"),
        format!(
            "local env = prova.topology(\"orders\", function(ctx)\n\
             \x20 ctx:defer(function()\n\
             \x20\x20  local f = io.open({marker:?}, \"a\"); if f then f:write(\"down\\n\"); f:close() end\n\
             \x20 end)\n\
             \x20 return {{ db = {{ url = \"postgres://127.0.0.1:5432/orders\" }},\n\
             \x20\x20\x20\x20\x20\x20   web = {{ url = \"http://127.0.0.1:8080\" }} }}\n\
             end)\n\
             prova.test(\"smoke\", function(t) t:expect(t:use(env).web.url):matches(\"^http\") end)\n",
            marker = marker.to_string_lossy(),
        ),
    )
    .unwrap();

    // start (detached) — returns once the topology self-registers.
    let out = prova(&root, &["start", "orders"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("started"), "start stdout: {stdout}");
    assert!(
        stdout.contains("http://127.0.0.1:8080"),
        "endpoints not printed: {stdout}"
    );

    // ps — the topology is listed as running with its endpoints.
    let ps = prova(&root, &["ps"]);
    let ps_out = String::from_utf8_lossy(&ps.stdout);
    assert!(
        ps_out.contains("orders") && ps_out.contains("running"),
        "ps: {ps_out}"
    );
    assert!(
        ps_out.contains("postgres://127.0.0.1:5432/orders"),
        "ps endpoints: {ps_out}"
    );

    // down — signals the holder; the detached child runs teardown (marker) and the record is gone.
    let down = prova(&root, &["down", "orders"]);
    assert!(
        down.status.success(),
        "down failed: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    assert!(
        String::from_utf8_lossy(&down.stdout).contains("torn down"),
        "down stdout: {}",
        String::from_utf8_lossy(&down.stdout)
    );
    assert!(
        marker.is_file(),
        "teardown did not run in the detached child (no marker)"
    );

    // ps again — nothing running.
    let ps2 = prova(&root, &["ps"]);
    assert!(
        String::from_utf8_lossy(&ps2.stdout).contains("no topologies running"),
        "ps after down: {}",
        String::from_utf8_lossy(&ps2.stdout)
    );

    std::fs::remove_dir_all(&root).ok();
}
