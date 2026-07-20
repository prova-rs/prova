//! The plugin searcher: makes `require("name")` resolve Lua plugins from bundled first-party
//! modules and from local disk, so a plugin is authored exactly like the first-party recipes
//! (compose primitives, follow the namespacing grammar, `return` a namespace table).
//!
//! Resolution order, appended to `package.searchers` so it never shadows Lua's own searchers:
//!   1. `BUNDLED` — first-party modules embedded in the binary, reserved for the `prova.*` namespace.
//!   2. `named` — a plugin declared in `prova.toml` (`[plugins]`), resolved to an exact file (git
//!      checkouts are fetched into the cache and land here as a path). The manifest is authoritative.
//!   3. each dir on `PROVA_PLUGIN_PATH` (colon-separated), then any extra `roots` (e.g. the global
//!      `data_dir/plugins`), then `./.prova/plugins/` — each as `<root>/<a/b>.lua` then
//!      `<root>/<a/b>/init.lua`.
//!
//! A module name's dots map to path separators (`acme.rabbitmq` → `acme/rabbitmq.lua`). A miss
//! returns a string listing where we looked, so `require`'s aggregate error is actionable. The
//! searcher never downloads anything — resolution is always bundled code or an explicit local file
//! (git fetch happens earlier, in the CLI, into the cache; see docs/design/plugin-system.md § Safety).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mlua::{Lua, Value, Variadic};

/// First-party Lua modules compiled into the binary, resolvable by `require`. Reserved for the
/// `prova.*` namespace. This is where first-party recipes migrate as they move off `include_str!`
/// eager-injection and onto the loadable path (see docs/design/plugin-system.md § Dogfooding).
const BUNDLED: &[(&str, &str)] = &[
    (
        "prova.workspace",
        include_str!("plugins/prova/workspace.lua"),
    ),
    // The transport-agnostic programmable double (mock / proxy / spy, with an ordered event log).
    // The reusable heart of http.mock/grpc.mock for an in-process, Lua-driven boundary.
    ("prova.double", include_str!("plugins/prova/double.lua")),
];

/// Install the plugin searcher into `lua`'s `package.searchers`.
///
/// `named` maps a manifest-declared plugin name to an exact file (a local path, or a git checkout
/// the CLI already fetched into the cache). `namespaces` maps a plugin's *canonical* name to its
/// module root directory, so a multi-file plugin can `require("<canonical>.<sub>")` its own sibling
/// files (namespaced by canonical name, so it is stable regardless of the consumer's alias and never
/// collides with another plugin). `roots` are extra disk search roots (typically the global
/// `data_dir/plugins`); the built-in `PROVA_PLUGIN_PATH` and `./.prova/plugins` roots are always
/// searched too. All are cloned into the searcher closure (the Lua state is single-threaded).
pub(crate) fn install(
    lua: &Lua,
    roots: &[PathBuf],
    named: &BTreeMap<String, PathBuf>,
    namespaces: &BTreeMap<String, PathBuf>,
) -> mlua::Result<()> {
    let package: mlua::Table = lua.globals().get("package")?;
    let searchers: mlua::Table = package.get("searchers")?;

    let roots = roots.to_vec();
    let named = named.clone();
    let namespaces = namespaces.clone();
    let searcher = lua.create_function(move |lua, name: String| {
        resolve(lua, &name, &roots, &named, &namespaces)
    })?;
    // Append after the built-in searchers (preload + path-based), so a plugin never shadows them.
    searchers.set(searchers.raw_len() + 1, searcher)?;
    Ok(())
}

/// A `package.searchers` entry: return a *loader function* when found (Lua then calls it to get the
/// module value), or a *string* explaining where we looked when not found.
fn resolve(
    lua: &Lua,
    name: &str,
    roots: &[PathBuf],
    named: &BTreeMap<String, PathBuf>,
    namespaces: &BTreeMap<String, PathBuf>,
) -> mlua::Result<Value> {
    // 1. Bundled first-party modules.
    if let Some((_, src)) = BUNDLED.iter().find(|(n, _)| *n == name) {
        return Ok(Value::Function(bundled_loader(lua, name, src)?));
    }

    // 2. Manifest-declared plugins — the authoritative, pinned source.
    let mut tried: Vec<String> = Vec::new();
    if let Some(path) = named.get(name) {
        if path.is_file() {
            return Ok(Value::Function(disk_loader(lua, path)?));
        }
        tried.push(path.display().to_string());
    }

    // 3. Intra-plugin requires: `<canonical>.<sub>` resolves `<sub>` under the plugin's own root, so
    //    a multi-file plugin can require its siblings (namespaced by canonical name → collision-safe).
    if let Some((prefix, rest)) = name.split_once('.') {
        if let Some(dir) = namespaces.get(prefix) {
            let rel = rest.replace('.', "/");
            for candidate in [
                dir.join(format!("{rel}.lua")),
                dir.join(&rel).join("init.lua"),
            ] {
                if candidate.is_file() {
                    return Ok(Value::Function(disk_loader(lua, &candidate)?));
                }
                tried.push(candidate.display().to_string());
            }
        }
    }

    // 4. Disk roots: PROVA_PLUGIN_PATH, then the passed roots, then ./.prova/plugins. `acme.rabbitmq`
    //    → `acme/rabbitmq`.
    let rel = name.replace('.', "/");
    for root in disk_roots(roots) {
        for candidate in [
            root.join(format!("{rel}.lua")),
            root.join(&rel).join("init.lua"),
        ] {
            if candidate.is_file() {
                return Ok(Value::Function(disk_loader(lua, &candidate)?));
            }
            tried.push(candidate.display().to_string());
        }
    }

    // Not found: a string is how a searcher reports a miss; Lua aggregates these into require's error.
    let mut msg = format!("\n\tno prova plugin {name:?} (bundled, manifest, or on disk)");
    for path in tried {
        msg.push_str(&format!("\n\t\tno file '{path}'"));
    }
    Ok(Value::String(lua.create_string(&msg)?))
}

/// Loader for a bundled module: evaluate the embedded chunk and return its value.
fn bundled_loader(lua: &Lua, name: &str, src: &'static str) -> mlua::Result<mlua::Function> {
    let chunk_name = format!("@prova/plugin/{name}");
    lua.create_function(move |lua, _args: Variadic<Value>| {
        lua.load(src).set_name(&chunk_name).eval::<Value>()
    })
}

/// Loader for a disk module: read the file at require-time and evaluate it.
fn disk_loader(lua: &Lua, path: &Path) -> mlua::Result<mlua::Function> {
    let path = path.to_path_buf();
    let chunk_name = format!("@{}", path.display());
    lua.create_function(move |lua, _args: Variadic<Value>| {
        let src = std::fs::read_to_string(&path).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read plugin {}: {e}", path.display()))
        })?;
        lua.load(&src).set_name(&chunk_name).eval::<Value>()
    })
}

/// Disk search roots, in order: every dir on `PROVA_PLUGIN_PATH`, then the caller-supplied `extra`
/// roots (e.g. the global `data_dir/plugins`), then `./.prova/plugins`.
fn disk_roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(path) = std::env::var("PROVA_PLUGIN_PATH") {
        for dir in path.split(':').filter(|s| !s.is_empty()) {
            roots.push(PathBuf::from(dir));
        }
    }
    roots.extend(extra.iter().cloned());
    roots.push(PathBuf::from(".prova/plugins"));
    roots
}
