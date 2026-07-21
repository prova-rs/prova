//! The `[topologies]` bridge: a project registers a topology a plugin provides, and it becomes a
//! first-class topology — listed by `prova up`, standable by name — without any glue Lua. The
//! registration is sugar for `prova.topology(alias, require(plugin).factory)`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prova-topo-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn up_no_arg(cwd: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .arg("up")
        .output()
        .unwrap();
    let combined =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), combined)
}

/// A plugin that PROVIDES a topology factory (`site.web`), a project that REGISTERS it under a name,
/// and `prova up` lists that name — the registration bridged into a real topology. RED today:
/// `[topologies]` is unknown to the manifest, so nothing is registered and `up` sees no topologies.
#[test]
fn a_registered_plugin_topology_is_listed() {
    let dir = tmp("bridge");
    // The plugin: `require("site")` returns a namespace whose `web` field is a topology factory.
    write(
        &dir,
        ".prova/plugins/site/init.lua",
        "return { web = function(ctx) return { url = \"http://x\" } end }\n",
    );
    // A file under the run paths, so discovery has something to scan (the topology itself comes from
    // the manifest, not this file).
    write(
        &dir,
        "proofs/placeholder_test.lua",
        "prova.test(\"placeholder\", function(t) t:expect(1):equals(1) end)\n",
    );
    // The manifest registers the plugin's factory under a topology name.
    write(
        &dir,
        ".prova.toml",
        "[run]\n\
         paths = [\"proofs\"]\n\
         plugin_root = \".prova/plugins\"\n\
         \n\
         [topologies]\n\
         homepage = { plugin = \"site\", factory = \"web\" }\n",
    );

    let (ok, out) = up_no_arg(&dir);
    assert!(ok, "listing should succeed: {out}");
    assert!(
        out.contains("homepage"),
        "the registered topology `homepage` is listed: {out}"
    );

    // And it's addressable by name through the stand-up path, not just the listing path: asking for a
    // name that doesn't exist reports the registered one as available (proving `load_topology` execs
    // the registration too, without our test having to hold a live topology).
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&dir)
        .args(["up", "does-not-exist"])
        .output()
        .unwrap();
    let combined = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        combined.contains("homepage"),
        "the registered topology is available to the stand-up path: {combined}"
    );
}

/// A `[topologies]` entry pointing at a factory the plugin doesn't have fails loudly, naming the
/// entry and what it tried — not a silent nil topology.
#[test]
fn a_bad_factory_reference_fails_clearly() {
    let dir = tmp("badref");
    write(
        &dir,
        ".prova/plugins/site/init.lua",
        "return { web = function(ctx) return {} end }\n",
    );
    write(
        &dir,
        "proofs/p_test.lua",
        "prova.test(\"p\", function(t) t:expect(1):equals(1) end)\n",
    );
    write(
        &dir,
        ".prova.toml",
        "[run]\npaths = [\"proofs\"]\nplugin_root = \".prova/plugins\"\n\n\
         [topologies]\nbroken = { plugin = \"site\", factory = \"nope\" }\n",
    );
    let (ok, out) = up_no_arg(&dir);
    assert!(!ok, "a bad factory reference must fail: {out}");
    assert!(out.contains("broken"), "names the offending entry: {out}");
}
