//! The plugin searcher: makes `require("name")` resolve Lua plugins from bundled first-party
//! modules and from local disk, so a plugin is authored exactly like the first-party recipes
//! (compose primitives, follow the namespacing grammar, `return` a namespace table).
//!
//! Resolution order, appended to `package.searchers` so it never shadows Lua's own searchers:
//!   1. `BUNDLED` — first-party modules embedded in the binary, reserved for the `prova.*` namespace.
//!   2. `named` — a plugin declared in `prova.toml` (`[plugins]`), resolved to an exact file (git
//!      checkouts are fetched into the cache and land here as a path). The manifest is authoritative.
//!   3. the **declared** plugin root — `<root>/<a/b>.lua` then `<root>/<a/b>/init.lua`. The CLI
//!      derives it from the manifest's `[run] plugin_root`, resolved against the project root; an
//!      embedder passes it via `RunConfig::with_plugin_root`.
//!
//! # Everything is declared
//!
//! Discovery finds the manifest (`.prova.toml` / `prova.toml` / `prova/prova.toml` /
//! `.prova/prova.toml`); from there, **the manifest names every place a plugin may come from**. There
//! is no default root, no `PROVA_PLUGIN_PATH`, no cwd-relative fallback, and no per-user plugin
//! directory. Each of those used to be an answer to "where could this `require` have come from?" that
//! you could not get by reading the project — and a resolution path outside version control lets a
//! proof pass on one machine and fail in CI with nothing in the repo to explain it.
//!
//! The declared root is **one** directory, not a list. An ambient root exists for a single job —
//! "this project's own plugins, without naming each one" — and that is inherently one place. A plugin
//! from anywhere else takes a name and a pinned source in `[plugins]`, which is more explicit and more
//! reproducible than a second scanned directory; a list would only add a precedence question.
//!
//! The payoff is auditability: one file answers the question completely, which matters most when the
//! reader is an agent that cannot simply *know* the conventions. A project that declares no roots
//! resolves no ambient plugins, and the miss message says exactly that rather than looking like a
//! typo. Git sources in `[plugins]` are the other half — pinned, fetched into the cache, equally
//! reproducible from the manifest alone.
//!
//! A module name's dots map to path separators (`acme.rabbitmq` → `acme/rabbitmq.lua`). A miss
//! returns a string listing where we looked, so `require`'s aggregate error is actionable. The
//! searcher never downloads anything — resolution is always bundled code or an explicit local file
//! (git fetch happens earlier, in the CLI, into the cache; see docs/design/plugin-system.md § Safety).
//!
//! # Private dependencies (bundled + isolated)
//!
//! The four steps above are the *consumer's* namespace. A plugin may also declare its own private
//! dependencies in its `prova.toml` (`[plugins]`):
//!
//! ```toml
//! [plugins]
//! inner = { path = "deps/inner" }
//! ```
//!
//! Such a plugin's chunk runs with a `require` scoped to that map, so `require("inner")` resolves
//! *for the library* while a consumer that required only the library cannot reach `inner` at all.
//! Two properties make the isolation real rather than cosmetic:
//!
//! - Scoping happens at **load**, by binding the chunk's environment — not inside the searcher, which
//!   only ever receives a module *name* and could never tell who was asking. This is also why a
//!   dependency required lazily (inside a function, at test time) still resolves privately.
//! - A private dependency is cached by **path**, in a registry-side table. Putting it in the global
//!   `package.loaded` — which is keyed by *name* — would hand every consumer a way to reach it, which
//!   is precisely the leak this closes.
//!
//! Consequences worth knowing: two plugins may depend on different things under one short name
//! without colliding, and a private dependency must live inside its dependant (or in the cache), not
//! at the top of `.prova/plugins/` — a top-level directory there is a *project* plugin and is
//! globally requirable by design.

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
/// collides with another plugin). `roots` carries the declared plugin root — the manifest's
/// `[run] plugin_root`, already absolutised — and it is the *only* directory scanned: nothing
/// machine-global, nothing from the environment, nothing cwd-relative (see the module docs). A slice
/// rather than one path only because the embedder API (`with_plugin_root`) accumulates; the manifest
/// declares exactly one. All are cloned into the closure (the Lua state is single-threaded).
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

    // 4. The declared disk roots, in order. `acme.rabbitmq` → `acme/rabbitmq`.
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
    // Having nowhere to look is a different mistake from looking and missing, and it has a different
    // fix. Since plugin roots are declared rather than defaulted, an undeclared root would otherwise
    // present as an ordinary "not found" and send the reader hunting for a misspelled plugin.
    if disk_roots(roots).is_empty() {
        msg.push_str(
            "\n\t\t(no plugin root declared — add `plugin_root` to [run] in prova.toml, \
             e.g. plugin_root = \".prova/plugins\")",
        );
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
    lua.create_function(move |lua, _args: Variadic<Value>| load_module(lua, &path))
}

/// Evaluate a plugin file.
///
/// When the plugin declares private dependencies (`prova.toml [plugins]`), its chunk runs in
/// an environment whose `require` consults *its own* dependency map first. That is what makes a
/// library able to use a dependency the consumer cannot see: the name resolves for the library and
/// nowhere else.
fn load_module(lua: &Lua, path: &Path) -> mlua::Result<Value> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        mlua::Error::RuntimeError(format!("cannot read plugin {}: {e}", path.display()))
    })?;
    let chunk_name = format!("@{}", path.display());
    let chunk = lua.load(&src).set_name(&chunk_name);
    // Every plugin runs in its own environment now — falling through to the real globals, so it sees
    // and sets exactly what it always could, plus a per-plugin `plugin` table (its own location) and,
    // for a plugin with private dependencies, a scoped `require`.
    chunk.set_environment(plugin_env(lua, path)?).eval::<Value>()
}

/// The environment a plugin chunk runs in.
///
/// Reads *and writes* fall through to the real globals (so a plugin sees and can set exactly what it
/// always could — only name resolution and `plugin` differ), plus two per-plugin bindings:
///
/// - **`plugin.dir`** — the directory the plugin's own file lives in. This is what lets a plugin
///   locate *its own* repo's artifacts. `prova.root` is the CONSUMING package's root, so a plugin
///   reused cross-repo (`[plugins] x = { path = "../other/..." }`) cannot anchor on it — it would
///   resolve the consumer's `target/`, not its own. `plugin.dir` is always the plugin's real home, so
///   `plugin.dir .. "/../../../target/debug/tool"` finds the plugin's binary wherever it is consumed.
/// - **`require`** — shadowed by a scoped one *only* when the plugin declares private dependencies.
fn plugin_env(lua: &Lua, path: &Path) -> mlua::Result<mlua::Table> {
    let env = lua.create_table()?;
    // RAW set, deliberately: `__newindex` below forwards writes to the real globals, so a plain `set`
    // here would install these per-plugin bindings as *everyone's* (handing every consumer this
    // plugin's private `require`, or a stale `plugin`). (Ask how I know.)
    let info = lua.create_table()?;
    if let Some(dir) = path.parent() {
        info.raw_set("dir", dir.to_string_lossy().into_owned())?;
    }
    env.raw_set("plugin", info)?;
    let own = private_deps(path);
    if !own.is_empty() {
        env.raw_set("require", scoped_require(lua, own)?)?;
    }
    let meta = lua.create_table()?;
    meta.set("__index", lua.globals())?;
    meta.set("__newindex", lua.globals())?;
    env.set_metatable(Some(meta))?;
    Ok(env)
}

/// `require` bound to one plugin's private dependency map.
///
/// A private dependency is cached by resolved *path* in a registry-side table — never in the global
/// `package.loaded`, which is keyed by name and is exactly the thing that would leak it to consumers.
/// A name the plugin did not declare falls through to the ordinary global `require`, so bundled
/// modules, the stdlib, and the project's own declared plugins keep working.
fn scoped_require(lua: &Lua, own: BTreeMap<String, PathBuf>) -> mlua::Result<mlua::Function> {
    lua.create_function(move |lua, name: String| {
        let Some(entry) = own.get(&name) else {
            // Not one of this plugin's declared dependencies — ordinary resolution.
            let global: mlua::Function = lua.globals().get("require")?;
            return global.call::<Value>(name);
        };
        let key = entry.display().to_string();
        let cache = private_cache(lua)?;
        // A sentinel goes in before evaluation, mirroring how Lua's own `require` breaks import
        // cycles: a dependency that (transitively) requires its dependant gets the sentinel rather
        // than recursing forever.
        match cache.get::<Value>(key.as_str())? {
            Value::Nil => {}
            hit => return Ok(hit),
        }
        cache.set(key.as_str(), true)?;
        let value = load_module(lua, entry)?;
        cache.set(key.as_str(), value.clone())?;
        Ok(value)
    })
}

/// Registry key for the private module cache — deliberately *not* `package.loaded`.
const PRIVATE_LOADED: &str = "prova.private_loaded";

/// The private module table, created on first use.
fn private_cache(lua: &Lua) -> mlua::Result<mlua::Table> {
    if let Ok(Value::Table(t)) = lua.named_registry_value::<Value>(PRIVATE_LOADED) {
        return Ok(t);
    }
    let table = lua.create_table()?;
    lua.set_named_registry_value(PRIVATE_LOADED, &table)?;
    Ok(table)
}

/// A plugin's declared private dependencies, read from the `prova.toml` beside its entry.
///
/// Only `path` sources resolve here: the searcher never downloads (see the module docs), so a git
/// dependency has to be fetched earlier. Anything unparseable is treated as "no private deps" rather
/// than a hard error — a malformed manifest should not make an otherwise-working plugin unloadable,
/// and the require that then fails names what it could not find.
fn private_deps(entry: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Some(dir) = entry.parent() else {
        return out;
    };
    let Ok(src) = std::fs::read_to_string(dir.join("prova.toml")) else {
        return out;
    };
    let Ok(table) = src.parse::<toml::Table>() else {
        return out;
    };
    let Some(plugins) = table.get("plugins").and_then(toml::Value::as_table) else {
        return out;
    };
    for (name, source) in plugins {
        // `name = "some/path"` or `name = { path = "some/path" }`.
        let rel = match source {
            toml::Value::String(s) => Some(s.as_str()),
            toml::Value::Table(t) => t.get("path").and_then(toml::Value::as_str),
            _ => None,
        };
        let Some(rel) = rel else { continue };
        let root = dir.join(rel);
        // A dependency may be a file, or a directory holding `<name>.lua` / `init.lua`.
        let candidates = [
            root.clone(),
            root.join(format!("{name}.lua")),
            root.join("init.lua"),
        ];
        if let Some(found) = candidates.iter().find(|p| p.is_file()) {
            out.insert(name.clone(), found.clone());
        }
    }
    out
}

/// Disk search roots: exactly the ones the caller declared, in order — nothing else.
///
/// This function used to add an env var (`PROVA_PLUGIN_PATH`) and a cwd-relative `./.prova/plugins`.
/// Both are gone on purpose. A root that comes from the environment or the working directory cannot
/// be read off the project, so "where could this `require` have come from?" had four answers, three
/// of them invisible from the repo. Now the manifest's `[run] plugin_root` is the whole story, which
/// is what lets a reader — or an agent — audit resolution by reading one file.
fn disk_roots(declared: &[PathBuf]) -> Vec<PathBuf> {
    declared.to_vec()
}
