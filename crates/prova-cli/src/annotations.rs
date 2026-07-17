//! LuaLS IDE integration — making `require("postgres")` (and the whole `prova` DSL) complete and
//! type-check in a test author's editor, with zero manual wiring.
//!
//! Two stacked gaps are closed by one mechanism:
//!   1. prova's own annotations (`library/prova.lua`, `library/modules.lua`) were only wired up
//!      inside the prova repo — a consumer never got them.
//!   2. plugins shipped no annotations at all, so `require("postgres")` was opaque.
//!
//! On every run that has a manifest, prova refreshes a prova-owned annotation folder,
//! `<home>/annotations/`, containing the embedded core stubs plus each resolved plugin's
//! `library/*.lua` `---@meta` stub. A single `.luarc.json` at the project root points LuaLS at that
//! folder, so adding a plugin to `prova.toml` makes its completions appear with no `.luarc.json`
//! edit — the pointer never moves, only the folder's contents.
//!
//! The folder is always refreshed (prova-owned, gitignored via `annotations/.gitignore`). Only the
//! *pointer* is gated by the `[luals] manage` policy, which auto-detects project type: a non-Lua
//! project has no `.luarc.json` (prova sets it up); a Lua-native project already owns one (prova
//! stays polite).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::home::Home;
use crate::manifest::Manage;

/// The embedded core annotation stubs (authoritative for the `prova` DSL surface), shipped into the
/// project so consumers get completion without a checkout of the prova repo.
///
/// Embedded **once**, in `prova_core::help`, and consumed twice: here (→ the IDE annotation folder,
/// for a human in an editor) and by `prova.help()` (→ structured data, for an agent driving the
/// environment). One source, two sinks — a second copy would drift, and the stub is the copy that
/// cannot be allowed to rot. See docs/design/agent-ergonomics.md §0.
use prova_core::help::CORE_STUBS;

/// What `setup` did, so the caller can print a concise, honest one-liner.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Plugin names whose `library/` stub was synced.
    pub synced_plugins: Vec<String>,
    /// `.luarc.json` was created (didn't exist before).
    pub luarc_created: bool,
    /// `.luarc.json` existed and lacked our entry, and policy was `Auto` — we left it alone. The
    /// caller prints the "run `prova init`" hint.
    pub luarc_hint: bool,
}

/// Refresh the annotation folder and manage the `.luarc.json` pointer per `manage`. Idempotent.
pub fn setup(
    home: &Home,
    roots: &BTreeMap<String, PathBuf>,
    manage: Manage,
) -> Result<Outcome, String> {
    let synced_plugins = sync_folder(home, roots)?;
    let mut outcome = Outcome {
        synced_plugins,
        ..Default::default()
    };

    let luarc = home.root.join(".luarc.json");
    let exists = luarc.is_file();
    match (manage, exists) {
        // Never touch the pointer; the folder is still synced above.
        (Manage::Never, _) => {}
        // Create when absent (prova owns config here — non-Lua project, or a fresh one).
        (Manage::Auto, false) | (Manage::Always, false) => {
            write_fresh_luarc(&luarc, &library_entry(home))?;
            outcome.luarc_created = true;
        }
        // Present + Auto: this project already owns a .luarc.json (Lua-native) — stay a polite guest.
        (Manage::Auto, true) => {
            if !luarc_has_entry(&luarc, &library_entry(home)) {
                outcome.luarc_hint = true;
            }
        }
        // Present + Always: the explicit opt-in — merge our two keys in.
        (Manage::Always, true) => {
            merge_luarc(&luarc, &library_entry(home))?;
        }
    }
    Ok(outcome)
}

/// Force-create/merge the `.luarc.json` pointer regardless of policy, and sync the folder — the
/// behavior of `prova init`, which is the user explicitly asking prova to wire up IDE support.
pub fn init(home: &Home, roots: &BTreeMap<String, PathBuf>) -> Result<Outcome, String> {
    let synced_plugins = sync_folder(home, roots)?;
    let luarc = home.root.join(".luarc.json");
    let entry = library_entry(home);
    let created = if luarc.is_file() {
        merge_luarc(&luarc, &entry)?;
        false
    } else {
        write_fresh_luarc(&luarc, &entry)?;
        true
    };
    Ok(Outcome {
        synced_plugins,
        luarc_created: created,
        luarc_hint: false,
    })
}

/// Write `<home>/annotations/`: the embedded core stubs, plus each resolved plugin's `library/*.lua`
/// under `plugins/`. The folder is emptied first so a removed plugin's stub doesn't linger. Returns
/// the names of plugins whose stubs were synced.
fn sync_folder(home: &Home, roots: &BTreeMap<String, PathBuf>) -> Result<Vec<String>, String> {
    let annotations = home.dir.join("annotations");
    // Recreate cleanly so stale stubs (a dropped plugin) don't persist.
    if annotations.exists() {
        std::fs::remove_dir_all(&annotations)
            .map_err(|e| format!("cannot clear {}: {e}", annotations.display()))?;
    }
    let plugins_dir = annotations.join("plugins");
    std::fs::create_dir_all(&plugins_dir)
        .map_err(|e| format!("cannot create {}: {e}", plugins_dir.display()))?;

    // prova fully owns this dir — gitignore it in-place so a stray commit can't capture generated
    // files, without editing any of the user's own .gitignore.
    std::fs::write(
        annotations.join(".gitignore"),
        "# generated by prova — do not edit\n*\n",
    )
    .map_err(|e| format!("cannot write annotations/.gitignore: {e}"))?;

    for (name, body) in CORE_STUBS {
        std::fs::write(annotations.join(name), body)
            .map_err(|e| format!("cannot write annotations/{name}: {e}"))?;
    }

    let mut synced = Vec::new();
    for (canonical, root) in roots {
        let lib = root.join("library");
        if !lib.is_dir() {
            continue;
        }
        let mut any = false;
        for entry in
            std::fs::read_dir(&lib).map_err(|e| format!("cannot read {}: {e}", lib.display()))?
        {
            let entry = entry.map_err(|e| format!("cannot read {}: {e}", lib.display()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                let dest = plugins_dir.join(entry.file_name());
                std::fs::copy(&path, &dest)
                    .map_err(|e| format!("cannot copy {}: {e}", path.display()))?;
                any = true;
            }
        }
        if any {
            synced.push(canonical.clone());
        }
    }
    synced.sort();
    Ok(synced)
}

/// The `workspace.library` entry pointing from the project root at the annotations folder, as a
/// forward-slash relative path (`annotations`, `.prova/annotations`, or `prova/annotations`).
fn library_entry(home: &Home) -> String {
    let rel = home.dir.strip_prefix(&home.root).unwrap_or(Path::new(""));
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.push("annotations".to_string());
    parts.join("/")
}

/// A fresh minimal `.luarc.json` for a project prova owns the config of.
fn write_fresh_luarc(path: &Path, library: &str) -> Result<(), String> {
    let doc = json!({
        "runtime.version": "Lua 5.4",
        "workspace.library": [library],
        "workspace.checkThirdParty": false,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Does an existing `.luarc.json` already list our library entry? A parse failure (comments / invalid
/// JSON) counts as "no" — we won't claim it's wired when we can't confirm it.
fn luarc_has_entry(path: &Path, library: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    match map.get("workspace.library") {
        Some(Value::Array(items)) => items.iter().any(|v| v.as_str() == Some(library)),
        _ => false,
    }
}

/// Merge our two keys into an existing `.luarc.json`: append the library entry if absent, and set
/// `runtime.version` only if unset (never override the user's). Non-destructive to other keys.
/// Errors (rather than corrupts) if the file isn't parseable JSON — the caller can surface a hint.
fn merge_luarc(path: &Path, library: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut doc: Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} is not plain JSON ({e}); add {library:?} to workspace.library by hand, \
             or set [luals] manage = \"never\"",
            path.display()
        )
    })?;
    let map = doc
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;

    // workspace.library: ensure our entry is present (create the array if needed).
    match map
        .entry("workspace.library".to_string())
        .or_insert_with(|| json!([]))
    {
        Value::Array(items) => {
            if !items.iter().any(|v| v.as_str() == Some(library)) {
                items.push(json!(library));
            }
        }
        other => *other = json!([library]),
    }
    // runtime.version: only if the user hasn't set it.
    map.entry("runtime.version".to_string())
        .or_insert_with(|| json!("Lua 5.4"));

    write_json(path, map)
}

fn write_json(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(&Value::Object(map.clone())).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let dir = std::env::temp_dir().join(format!("prova-anno-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn home_at(root: &Path, sub: Option<&str>) -> Home {
        let dir = match sub {
            Some(s) => root.join(s),
            None => root.to_path_buf(),
        };
        std::fs::create_dir_all(&dir).unwrap();
        Home {
            root: root.to_path_buf(),
            dir: dir.clone(),
            manifest: dir.join("prova.toml"),
        }
    }

    /// A fake plugin dir with a `library/<name>.lua` stub.
    fn plugin_with_stub(base: &Path, name: &str) -> PathBuf {
        let root = base.join(format!("plugin-{name}"));
        std::fs::create_dir_all(root.join("library")).unwrap();
        std::fs::write(
            root.join("library").join(format!("{name}.lua")),
            format!("---@meta {name}\nlocal M = {{}}\nreturn M\n"),
        )
        .unwrap();
        root
    }

    #[test]
    fn library_entry_reflects_home_location() {
        let root = Path::new("/proj");
        assert_eq!(library_entry(&home_flat(root)), "annotations");
        assert_eq!(
            library_entry(&home_sub(root, ".prova")),
            ".prova/annotations"
        );
        assert_eq!(library_entry(&home_sub(root, "prova")), "prova/annotations");
    }
    fn home_flat(root: &Path) -> Home {
        Home {
            root: root.into(),
            dir: root.into(),
            manifest: root.join("prova.toml"),
        }
    }
    fn home_sub(root: &Path, sub: &str) -> Home {
        Home {
            root: root.into(),
            dir: root.join(sub),
            manifest: root.join(sub).join("prova.toml"),
        }
    }

    #[test]
    fn sync_writes_core_stubs_plugin_stubs_and_gitignore() {
        let t = Tmp::new("sync");
        let home = home_at(&t.0, Some(".prova"));
        let mut roots = BTreeMap::new();
        roots.insert("postgres".to_string(), plugin_with_stub(&t.0, "postgres"));

        let synced = sync_folder(&home, &roots).unwrap();
        assert_eq!(synced, vec!["postgres".to_string()]);
        let anno = home.dir.join("annotations");
        assert!(anno.join("prova.lua").is_file());
        assert!(anno.join("modules.lua").is_file());
        assert!(anno.join("plugins/postgres.lua").is_file());
        assert_eq!(
            std::fs::read_to_string(anno.join(".gitignore"))
                .unwrap()
                .trim_end(),
            "# generated by prova — do not edit\n*".trim_end()
        );
    }

    #[test]
    fn sync_is_clean_dropping_a_removed_plugin() {
        let t = Tmp::new("drop");
        let home = home_at(&t.0, None);
        let mut roots = BTreeMap::new();
        roots.insert("redis".to_string(), plugin_with_stub(&t.0, "redis"));
        sync_folder(&home, &roots).unwrap();
        assert!(home.dir.join("annotations/plugins/redis.lua").is_file());

        // Re-sync with no plugins → the stale stub is gone.
        sync_folder(&home, &BTreeMap::new()).unwrap();
        assert!(!home.dir.join("annotations/plugins/redis.lua").exists());
        assert!(home.dir.join("annotations/prova.lua").is_file());
    }

    #[test]
    fn auto_creates_luarc_when_absent() {
        let t = Tmp::new("auto-absent");
        let home = home_at(&t.0, None);
        let out = setup(&home, &BTreeMap::new(), Manage::Auto).unwrap();
        assert!(out.luarc_created);
        let text = std::fs::read_to_string(t.0.join(".luarc.json")).unwrap();
        assert!(text.contains("\"annotations\""), "{text}");
        assert!(text.contains("Lua 5.4"), "{text}");
    }

    #[test]
    fn auto_is_polite_when_luarc_exists() {
        let t = Tmp::new("auto-present");
        let home = home_at(&t.0, None);
        std::fs::write(
            t.0.join(".luarc.json"),
            "{ \"diagnostics.globals\": [\"vim\"] }",
        )
        .unwrap();
        let out = setup(&home, &BTreeMap::new(), Manage::Auto).unwrap();
        assert!(!out.luarc_created);
        assert!(out.luarc_hint);
        // The user's file is untouched.
        let text = std::fs::read_to_string(t.0.join(".luarc.json")).unwrap();
        assert!(
            text.contains("vim") && !text.contains("annotations"),
            "{text}"
        );
    }

    #[test]
    fn always_merges_into_existing_luarc_nondestructively() {
        let t = Tmp::new("always");
        let home = home_at(&t.0, Some(".prova"));
        std::fs::write(
            t.0.join(".luarc.json"),
            "{ \"runtime.version\": \"Lua 5.3\", \"diagnostics.globals\": [\"vim\"], \"workspace.library\": [\"types\"] }",
        )
        .unwrap();
        setup(&home, &BTreeMap::new(), Manage::Always).unwrap();
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(t.0.join(".luarc.json")).unwrap())
                .unwrap();
        // Our entry appended; user's library entry + version + other keys preserved.
        let lib = doc["workspace.library"].as_array().unwrap();
        assert!(lib.iter().any(|v| v == ".prova/annotations"));
        assert!(lib.iter().any(|v| v == "types"));
        assert_eq!(doc["runtime.version"], "Lua 5.3"); // not overridden
        assert_eq!(doc["diagnostics.globals"][0], "vim");
    }

    #[test]
    fn never_syncs_folder_but_leaves_luarc() {
        let t = Tmp::new("never");
        let home = home_at(&t.0, None);
        let out = setup(&home, &BTreeMap::new(), Manage::Never).unwrap();
        assert!(!out.luarc_created && !out.luarc_hint);
        assert!(!t.0.join(".luarc.json").exists());
        assert!(home.dir.join("annotations/prova.lua").is_file()); // folder still synced
    }

    #[test]
    fn init_creates_or_merges() {
        let t = Tmp::new("init");
        let home = home_at(&t.0, None);
        let out = init(&home, &BTreeMap::new()).unwrap();
        assert!(out.luarc_created);
        // Second init merges (idempotent, still has our entry once).
        let out2 = init(&home, &BTreeMap::new()).unwrap();
        assert!(!out2.luarc_created);
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(t.0.join(".luarc.json")).unwrap())
                .unwrap();
        let count = doc["workspace.library"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| *v == "annotations")
            .count();
        assert_eq!(count, 1);
    }
}
