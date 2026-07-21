//! `[topologies].<name>.options` — a manifest-registered topology passes options to the factory as
//! its second argument (`require(plugin).factory(ctx, <options>)`). This is what lets a topology like
//! the `parallels` `vm` carry the one thing the caller can't otherwise supply — the base VM template
//! to clone — from the manifest rather than a hard-coded default. Absent options, the factory is
//! registered bare and receives only `ctx` (covered by the bridge tests).
//!
//! The observable: a factory that fails with what it received embedded, so `up` never holds a live
//! topology and the error carries the proof.

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

fn up(cwd: &Path, name: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(cwd)
        .args(["up", name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
}

/// The core contract: a string option declared in the manifest arrives as `opts.image` in the factory.
/// Without the feature the factory's second argument is nil, so `opts.image` would be `nil`, not the
/// declared value.
#[test]
fn a_string_option_reaches_the_factory() {
    let dir = tmp("opt-str");
    write(
        &dir,
        ".prova/plugins/site/init.lua",
        "return { web = function(ctx, opts) error(\"image=\" .. tostring(opts and opts.image)) end }\n",
    );
    write(
        &dir,
        "proofs/p_test.lua",
        "prova.test(\"p\", function(t) t:expect(1):equals(1) end)\n",
    );
    write(
        &dir,
        ".prova.toml",
        "[run]\nproofs = [\"proofs\"]\nplugin_root = \".prova/plugins\"\n\n\
         [topologies]\n\
         site = { plugin = \"site\", factory = \"web\", options = { image = \"ubuntu-24.04\" } }\n",
    );
    let out = up(&dir, "site");
    assert!(
        out.contains("image=ubuntu-24.04"),
        "the factory received the manifest options as its second argument: {out}"
    );
}

/// The serializer round-trips typed and nested values — an integer, a bool, and a nested table — not
/// just flat strings, so a factory can take structured options (`{ cpus = 2, wait = { ssh = true } }`).
#[test]
fn typed_and_nested_options_round_trip() {
    let dir = tmp("opt-nested");
    write(
        &dir,
        ".prova/plugins/site/init.lua",
        "return { web = function(ctx, opts)\n\
         error(\"cpus=\" .. tostring(opts.cpus) .. \" ssh=\" .. tostring(opts.wait.ssh))\n\
         end }\n",
    );
    write(
        &dir,
        "proofs/p_test.lua",
        "prova.test(\"p\", function(t) t:expect(1):equals(1) end)\n",
    );
    write(
        &dir,
        ".prova.toml",
        "[run]\nproofs = [\"proofs\"]\nplugin_root = \".prova/plugins\"\n\n\
         [topologies]\n\
         site = { plugin = \"site\", factory = \"web\", options = { cpus = 2, wait = { ssh = true } } }\n",
    );
    let out = up(&dir, "site");
    assert!(
        out.contains("cpus=2 ssh=true"),
        "integer, bool, and nested-table options survive serialization: {out}"
    );
}

/// A string option carrying Lua metacharacters (quotes, backslash) is escaped, not interpolated as
/// code — the manifest can never inject into the synthesized registration.
#[test]
fn option_strings_are_escaped_not_injected() {
    let dir = tmp("opt-escape");
    write(
        &dir,
        ".prova/plugins/site/init.lua",
        "return { web = function(ctx, opts) error(\"got:\" .. opts.name) end }\n",
    );
    write(
        &dir,
        "proofs/p_test.lua",
        "prova.test(\"p\", function(t) t:expect(1):equals(1) end)\n",
    );
    // A value that, spliced raw, would close the string and run code. Escaped, it's just data.
    write(
        &dir,
        ".prova.toml",
        "[run]\nproofs = [\"proofs\"]\nplugin_root = \".prova/plugins\"\n\n\
         [topologies]\n\
         site = { plugin = \"site\", factory = \"web\", options = { name = \"a\\\") os.exit(1) --\" } }\n",
    );
    let out = up(&dir, "site");
    assert!(
        out.contains("got:a\") os.exit(1) --"),
        "the metacharacter-laden option arrived verbatim as data, not executed: {out}"
    );
}
