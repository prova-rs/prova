//! LuaLS IDE integration — making `require("postgres")` (and the whole `prova` DSL) complete and
//! type-check in a test author's editor, with zero manual wiring.
//!
//! Two stacked gaps are closed by one mechanism:
//!   1. prova's own annotations (`library/prova.lua`, `library/modules.lua`) were only wired up
//!      inside the prova repo — a consumer never got them.
//!   2. plugins shipped no annotations at all, so `require("postgres")` was opaque.
//!
//! `.luarc.json`'s `workspace.library` lists the annotation sources **directly**:
//!
//! - `<data>/lua/annotations/` — the core stubs, written out of the binary to a stable, unversioned
//!   path shared by every project on the machine (a `.version` stamp keeps them fresh across
//!   upgrades). The stable path is what lets the `.luarc.json` entry be written once and never churn;
//! - each resolved plugin's own `library/` dir — the checkout under `<cache>/plugins/`, referenced
//!   in place.
//!
//! Nothing is copied, and **no per-project state is stored outside the project.** That is the whole
//! design: the only project-specific fact is *which* plugins are used, and `prova.toml` already
//! records that, inside the repo. An earlier iteration bundled the selection into a per-project
//! "view" directory in the cache — which bought nothing (every element was already shared) but
//! created a two-way consistency problem, since a cache directory keyed by a project must be
//! garbage-collected when that project disappears. Referencing the shared paths directly means
//! there is nothing to orphan and therefore nothing to collect.
//!
//! The cost of the direct list is that the entry set changes when the plugin set does, so
//! `.luarc.json` must be reconciled rather than left alone. prova rewrites files it created and
//! **merges non-destructively into user-owned files** — quietly, under the default `auto` — so
//! editor wiring just works with zero ceremony. Every write is change-gated (identical content is
//! never rewritten), and the caller narrates only actual changes: a steady-state run says nothing.
//! The one file prova will not touch is one it cannot parse as plain JSON (JSONC/comments) — there
//! it hints instead. `manage = "never"` opts a project out entirely (the right setting when a
//! repo deliberately commits a hand-maintained `.luarc.json`, which machine-local absolute entries
//! would otherwise keep dirtying).
//!
//! The only project-local artifact is `.luarc.json` itself. It holds machine-local absolute paths, so
//! it isn't shareable and shouldn't be committed — `prova init` says so once and leaves the
//! `.gitignore` decision to the user.
//!
//! Only the pointer is gated by the `[luals] manage` policy, which auto-detects project type: a
//! non-Lua project has no `.luarc.json` (prova sets it up); a Lua-native project already owns one
//! (prova stays polite).

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
use prova_core::SystemLayout;

/// What `setup` did, so the caller can print a concise, honest one-liner — and stay SILENT when
/// nothing changed (this runs on every invocation; a steady-state run must not narrate).
#[derive(Debug, Default)]
pub struct Outcome {
    /// Plugin names whose `library/` dir is listed in `workspace.library`.
    pub linked_plugins: Vec<String>,
    /// Where the shared core stubs live, for callers that want to name it.
    pub core_dir: PathBuf,
    /// `.luarc.json` was created (didn't exist before).
    pub luarc_created: bool,
    /// `.luarc.json` existed and its entry list actually changed this run (a merge or refresh
    /// that wrote bytes). False in the steady state, even though setup ran.
    pub luarc_updated: bool,
    /// `.luarc.json` exists but is not plain JSON (comments / JSONC), so prova cannot merge its
    /// entries safely. The caller surfaces how to wire it by hand.
    pub luarc_hint: bool,
}

/// Refresh the annotation view and manage the `.luarc.json` pointer per `manage`. Idempotent.
pub fn setup(
    home: &Home,
    roots: &BTreeMap<String, PathBuf>,
    manage: Manage,
    layout: &dyn SystemLayout,
    version: &str,
) -> Result<Outcome, String> {
    let core_dir = install_core_stubs(layout, version)?;
    let (entries, linked_plugins) = library_entries(&core_dir, roots);
    let mut outcome = Outcome {
        linked_plugins,
        core_dir,
        ..Default::default()
    };

    // The editor pointer goes at the home dir — which IS the project root the editor opens (the parent
    // of a nested `.prova/`/`prova/`), so LuaLS actually finds it.
    let luarc = home.dir.join(".luarc.json");
    let exists = luarc.is_file();
    match (manage, exists) {
        // Never touch the pointer; the core stubs are still installed above.
        (Manage::Never, _) => {}
        // Create when absent (non-Lua project, or a fresh one).
        (Manage::Auto, false) | (Manage::Always, false) => {
            write_fresh_luarc(&luarc, &entries)?;
            outcome.luarc_created = true;
        }
        // Present + Auto (the default): reconcile quietly. A file prova wrote is refreshed to the
        // canonical shape; a user-owned file gets prova's entries merged NON-destructively
        // (`merge_luarc` preserves every key and entry it doesn't manage). Editor wiring should
        // just work — the one file prova cannot merge safely (JSONC / comments) downgrades to a
        // hint rather than an error, because nothing is broken about the *run*.
        (Manage::Auto, true) => {
            if luarc_is_ours(&luarc) {
                outcome.luarc_updated = write_fresh_luarc(&luarc, &entries)?;
            } else {
                match merge_luarc(&luarc, &entries, layout) {
                    Ok(changed) => outcome.luarc_updated = changed,
                    Err(_) => outcome.luarc_hint = true,
                }
            }
        }
        // Present + Always: the explicit opt-in — same reconcile, but an unmergeable file is a
        // real error (the user asked for a merge that cannot happen).
        (Manage::Always, true) => {
            outcome.luarc_updated = merge_luarc(&luarc, &entries, layout)?;
        }
    }
    Ok(outcome)
}

/// Write the embedded core stubs to the **stable** `<data>/lua/annotations/` dir and return it.
/// Shared by every project on the machine; the path carries no version segment, so a project's
/// `.luarc.json` entry is written once and never churns across upgrades.
///
/// Each stub is written only when its bytes differ, so the steady state is a few `read`s and no
/// write (this runs on every invocation). Freshness across upgrades rides a `.version` stamp: on a
/// version change (or first install) we reclaim any stub we no longer ship — the stable dir outlives
/// any single version, so an orphan would otherwise linger — and advance the stamp.
fn install_core_stubs(layout: &dyn SystemLayout, version: &str) -> Result<PathBuf, String> {
    let dir = layout.annotations_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    for (name, body) in CORE_STUBS {
        let path = dir.join(name);
        let current = std::fs::read(&path).ok();
        if current.as_deref() != Some(body.as_bytes()) {
            std::fs::write(&path, body)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
    }
    let stamp = dir.join(".version");
    if std::fs::read_to_string(&stamp).ok().as_deref() != Some(version) {
        let shipped: std::collections::HashSet<&str> = CORE_STUBS.iter().map(|(n, _)| *n).collect();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".lua") && !shipped.contains(name.as_ref()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        std::fs::write(&stamp, version)
            .map_err(|e| format!("cannot write {}: {e}", stamp.display()))?;
    }
    Ok(dir)
}

/// The full `workspace.library` list: the shared core stubs, then each resolved plugin's `library/`
/// dir referenced in place. Returns the entries alongside the plugin names that contributed one (a
/// plugin shipping no `library/` simply doesn't appear).
fn library_entries(
    core_dir: &Path,
    roots: &BTreeMap<String, PathBuf>,
) -> (Vec<String>, Vec<String>) {
    let mut entries = vec![path_entry(core_dir)];
    let mut linked = Vec::new();
    for (canonical, root) in roots {
        let lib = root.join("library");
        if lib.is_dir() {
            entries.push(path_entry(&lib));
            linked.push(canonical.clone());
        }
    }
    linked.sort();
    (entries, linked)
}

/// Is a `workspace.library` entry one prova manages? True for anything under the annotations dir or
/// the plugin checkout cache, which covers every entry prova emits for a cached plugin.
///
/// A plugin resolved from a **local path** is deliberately not matched: its `library/` lives wherever
/// the user keeps it, indistinguishable from a hand-added entry. The consequence is that dropping a
/// local plugin leaves its (still valid, still existing) path in `workspace.library` until the user
/// removes it — strictly better than the alternative failure, deleting an entry the user added.
fn is_managed(entry: &str, layout: &dyn SystemLayout) -> bool {
    [layout.annotations_dir(), layout.plugin_cache_dir()]
        .iter()
        .any(|root| entry.starts_with(&path_entry(root)))
}

/// A `workspace.library` entry for an absolute path, forward-slashed so the JSON reads the same on
/// every platform (LuaLS normalizes separators itself).
fn path_entry(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The exact key set prova writes into a file it owns. Used to recognize its own handiwork.
const OUR_KEYS: [&str; 3] = [
    "runtime.version",
    "workspace.library",
    "workspace.checkThirdParty",
];

/// A fresh minimal `.luarc.json` for a project prova owns the config of. Returns whether bytes
/// actually changed — the steady state (this runs every invocation) is a read and no write.
fn write_fresh_luarc(path: &Path, library: &[String]) -> Result<bool, String> {
    let doc = json!({
        "runtime.version": "Lua 5.4",
        "workspace.library": library,
        "workspace.checkThirdParty": false,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())? + "\n";
    if std::fs::read_to_string(path).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    std::fs::write(path, text)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

/// Did prova write this `.luarc.json`? True only when the file carries *exactly* prova's own keys and
/// nothing else — the shape `write_fresh_luarc` produces.
///
/// This is what lets the entry list stay current under `auto`. Because the list now tracks the plugin
/// set, a file prova created must be refreshed when that set changes; a file the user owns must not
/// be. Requiring an exact key-set match errs toward "not ours": add one setting of your own and prova
/// treats the file as yours from then on, hinting rather than rewriting.
fn luarc_is_ours(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    map.len() == OUR_KEYS.len() && OUR_KEYS.iter().all(|k| map.contains_key(*k))
}

/// Reconcile our entries into an existing `.luarc.json`: drop prova-managed entries that are no
/// longer current (a dropped plugin, a previous version's core stubs), add the ones that are missing,
/// and leave every entry we don't manage untouched. `runtime.version` is set only if unset — never
/// overriding the user's. Non-destructive to other keys. Returns whether bytes actually changed —
/// a file already carrying the current entry set is left untouched (no rewrite, no mtime churn).
///
/// Errors (rather than corrupts) if the file isn't parseable JSON — the caller can surface a hint.
fn merge_luarc(path: &Path, library: &[String], layout: &dyn SystemLayout) -> Result<bool, String> {
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

    match map
        .entry("workspace.library".to_string())
        .or_insert_with(|| json!([]))
    {
        Value::Array(items) => {
            // Sweep out our own stale entries first, so a dropped plugin doesn't accumulate.
            items.retain(|v| match v.as_str() {
                Some(s) => !is_managed(s, layout) || library.iter().any(|w| w == s),
                None => true,
            });
            for want in library {
                if !items.iter().any(|v| v.as_str() == Some(want.as_str())) {
                    items.push(json!(want));
                }
            }
        }
        other => *other = json!(library),
    }
    // runtime.version: only if the user hasn't set it.
    map.entry("runtime.version".to_string())
        .or_insert_with(|| json!("Lua 5.4"));

    write_json_if_changed(path, map, &text)
}

fn write_json_if_changed(
    path: &Path,
    map: &Map<String, Value>,
    original: &str,
) -> Result<bool, String> {
    let text =
        serde_json::to_string_pretty(&Value::Object(map.clone())).map_err(|e| e.to_string())?
            + "\n";
    if text == original {
        return Ok(false);
    }
    std::fs::write(path, text)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
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

    fn home_at(base: &Path, sub: Option<&str>) -> Home {
        let dir = match sub {
            Some(s) => base.join(s),
            None => base.to_path_buf(),
        };
        std::fs::create_dir_all(&dir).unwrap();
        Home {
            manifest: dir.join("prova.toml"),
            dir,
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

    /// A layout whose cache/data/config all sit under the test's temp dir.
    fn layout_at(root: &Path) -> prova_core::RootedSystemLayout {
        prova_core::RootedSystemLayout::new(root.join("sys"))
    }

    const V: &str = "0.0.0-test";

    /// The library list must name the shared core dir and each plugin's *own* `library/` — no copy,
    /// no per-project directory anywhere.
    #[test]
    fn entries_point_at_shared_sources_and_nothing_is_project_local() {
        let t = Tmp::new("entries");
        let home = home_at(&t.0, Some(".prova"));
        let layout = layout_at(&t.0);
        let plugin = plugin_with_stub(&t.0, "postgres");
        let mut roots = BTreeMap::new();
        roots.insert("postgres".to_string(), plugin.clone());

        let core = install_core_stubs(&layout, V).unwrap();
        let (entries, linked) = library_entries(&core, &roots);

        assert_eq!(linked, vec!["postgres".to_string()]);
        assert_eq!(
            entries,
            vec![path_entry(&core), path_entry(&plugin.join("library"))]
        );
        assert!(core.join("prova.lua").is_file());
        assert!(core.join("modules.lua").is_file());
        // The plugin's stub is referenced where it already lives — never duplicated.
        assert!(plugin.join("library/postgres.lua").is_file());
        // Nothing whatsoever was written into the project.
        assert!(!home.dir.join("annotations").exists());
    }

    /// Core stubs are shared across projects and across versions in *one* stable dir — that is what
    /// removes the need for any per-project cache state (no back-pointer, no GC) and keeps the
    /// `.luarc.json` entry write-once. An upgrade refreshes the same dir in place (see the stamp
    /// test), rather than minting a new version-keyed path the pointer would then have to chase.
    #[test]
    fn core_stubs_share_one_stable_dir_across_versions() {
        let t = Tmp::new("shared");
        let layout = layout_at(&t.0);
        let one = install_core_stubs(&layout, V).unwrap();
        let two = install_core_stubs(&layout, V).unwrap();
        assert_eq!(one, two, "same version must resolve to one shared dir");
        let next = install_core_stubs(&layout, "9.9.9").unwrap();
        assert_eq!(
            one, next,
            "an upgrade refreshes the same stable dir, not a new one"
        );
        assert!(one.join("prova.lua").is_file());
    }

    /// A plugin shipping no `library/` contributes no entry (and isn't reported as linked).
    #[test]
    fn a_plugin_without_stubs_contributes_nothing() {
        let t = Tmp::new("nostub");
        let layout = layout_at(&t.0);
        let bare = t.0.join("plugin-bare");
        std::fs::create_dir_all(&bare).unwrap();
        let mut roots = BTreeMap::new();
        roots.insert("bare".to_string(), bare);

        let core = install_core_stubs(&layout, V).unwrap();
        let (entries, linked) = library_entries(&core, &roots);
        assert_eq!(entries, vec![path_entry(&core)]);
        assert!(linked.is_empty());
    }

    #[test]
    fn auto_creates_luarc_when_absent() {
        let t = Tmp::new("auto-absent");
        let home = home_at(&t.0, None);
        let layout = layout_at(&t.0);
        let out = setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        assert!(out.luarc_created);
        let text = std::fs::read_to_string(t.0.join(".luarc.json")).unwrap();
        assert!(text.contains(&path_entry(&out.core_dir)), "{text}");
        assert!(text.contains("Lua 5.4"), "{text}");
    }

    /// The default policy merges into a user-owned file quietly and non-destructively: prova's
    /// entries land, the user's keys survive, and the outcome reports an update (not a hint).
    #[test]
    fn auto_merges_into_a_user_owned_luarc_nondestructively() {
        let t = Tmp::new("auto-present");
        let home = home_at(&t.0, None);
        std::fs::write(
            t.0.join(".luarc.json"),
            "{ \"diagnostics.globals\": [\"vim\"] }",
        )
        .unwrap();
        let layout = layout_at(&t.0);
        let out = setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        assert!(!out.luarc_created);
        assert!(!out.luarc_hint);
        assert!(out.luarc_updated, "the merge is a real change");
        let text = std::fs::read_to_string(t.0.join(".luarc.json")).unwrap();
        assert!(text.contains("vim"), "user key clobbered: {text}");
        assert!(
            text.contains(&path_entry(&out.core_dir)),
            "our entry missing: {text}"
        );
    }

    /// The steady state is SILENT: once the entries are current, another run neither writes the
    /// file nor reports anything — this runs on every invocation, and must not narrate or churn.
    #[test]
    fn auto_is_silent_and_writeless_in_the_steady_state() {
        let t = Tmp::new("auto-steady");
        let home = home_at(&t.0, None);
        std::fs::write(
            t.0.join(".luarc.json"),
            "{ \"diagnostics.globals\": [\"vim\"] }",
        )
        .unwrap();
        let layout = layout_at(&t.0);
        setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        let after_first = std::fs::read_to_string(t.0.join(".luarc.json")).unwrap();

        let out = setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        assert!(!out.luarc_created && !out.luarc_updated && !out.luarc_hint);
        assert_eq!(
            std::fs::read_to_string(t.0.join(".luarc.json")).unwrap(),
            after_first,
            "steady state must not rewrite the file"
        );
    }

    /// A file prova cannot parse as plain JSON (JSONC comments) is left alone with a hint under
    /// `auto` — merging would corrupt it, and the run itself is fine.
    #[test]
    fn auto_hints_instead_of_touching_an_unparseable_luarc() {
        let t = Tmp::new("auto-jsonc");
        let home = home_at(&t.0, None);
        let jsonc = "// my config\n{ \"diagnostics.globals\": [\"vim\"] }";
        std::fs::write(t.0.join(".luarc.json"), jsonc).unwrap();
        let layout = layout_at(&t.0);
        let out = setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        assert!(out.luarc_hint);
        assert!(!out.luarc_created && !out.luarc_updated);
        assert_eq!(
            std::fs::read_to_string(t.0.join(".luarc.json")).unwrap(),
            jsonc,
            "an unparseable file must be left byte-identical"
        );
    }

    /// The direct list changes when the plugin set does, so a file prova wrote must be refreshed
    /// under `auto` — otherwise adding a plugin would silently fail to reach the editor.
    #[test]
    fn auto_refreshes_a_luarc_that_prova_wrote() {
        let t = Tmp::new("auto-refresh");
        let home = home_at(&t.0, None);
        let layout = layout_at(&t.0);
        let luarc = t.0.join(".luarc.json");

        setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        let plugin = plugin_with_stub(&t.0, "redis");
        let mut roots = BTreeMap::new();
        roots.insert("redis".to_string(), plugin.clone());
        let out = setup(&home, &roots, Manage::Auto, &layout, V).unwrap();

        assert!(
            !out.luarc_hint,
            "prova's own file should be refreshed, not hinted about"
        );
        let text = std::fs::read_to_string(&luarc).unwrap();
        assert!(
            text.contains(&path_entry(&plugin.join("library"))),
            "newly added plugin missing from the list: {text}"
        );

        // ...and dropping it again removes the entry, rather than accumulating.
        setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        let text = std::fs::read_to_string(&luarc).unwrap();
        assert!(
            !text.contains("plugin-redis"),
            "stale entry lingered: {text}"
        );
    }

    /// One user-added key is enough to make the file theirs — prova switches from the canonical
    /// rewrite to the non-destructive merge, so the user's key survives every later sync.
    #[test]
    fn a_user_edited_luarc_stops_being_ours() {
        let t = Tmp::new("ownership");
        let home = home_at(&t.0, None);
        let layout = layout_at(&t.0);
        let luarc = t.0.join(".luarc.json");

        setup(&home, &BTreeMap::new(), Manage::Auto, &layout, V).unwrap();
        assert!(luarc_is_ours(&luarc));

        let mut doc: Value =
            serde_json::from_str(&std::fs::read_to_string(&luarc).unwrap()).unwrap();
        doc.as_object_mut()
            .unwrap()
            .insert("diagnostics.globals".into(), json!(["vim"]));
        std::fs::write(&luarc, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        assert!(
            !luarc_is_ours(&luarc),
            "a user-added key must transfer ownership"
        );
        // A later sync (here: with a plugin added) merges rather than rewrites.
        let plugin = plugin_with_stub(&t.0, "redis");
        let mut roots = BTreeMap::new();
        roots.insert("redis".to_string(), plugin.clone());
        let out = setup(&home, &roots, Manage::Auto, &layout, V).unwrap();
        assert!(out.luarc_updated && !out.luarc_hint);
        let text = std::fs::read_to_string(&luarc).unwrap();
        assert!(text.contains("vim"), "the user's key was clobbered: {text}");
        assert!(
            text.contains(&path_entry(&plugin.join("library"))),
            "plugin entry not merged: {text}"
        );
    }

    #[test]
    fn always_merges_into_existing_luarc_nondestructively() {
        let t = Tmp::new("always");
        let home = home_at(&t.0, None);
        std::fs::write(
            t.0.join(".luarc.json"),
            "{ \"runtime.version\": \"Lua 5.3\", \"diagnostics.globals\": [\"vim\"], \"workspace.library\": [\"types\"] }",
        )
        .unwrap();
        let layout = layout_at(&t.0);
        let out = setup(&home, &BTreeMap::new(), Manage::Always, &layout, V).unwrap();
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(t.0.join(".luarc.json")).unwrap())
                .unwrap();
        // Our entry appended; user's library entry + version + other keys preserved.
        let lib = doc["workspace.library"].as_array().unwrap();
        let ours = path_entry(&out.core_dir);
        assert!(lib.iter().any(|v| v.as_str() == Some(ours.as_str())));
        assert!(lib.iter().any(|v| v == "types"));
        assert_eq!(doc["runtime.version"], "Lua 5.3"); // not overridden
        assert_eq!(doc["diagnostics.globals"][0], "vim");
    }

    /// Merging must reclaim prova's own stale entries while never touching the user's.
    #[test]
    fn always_sweeps_stale_managed_entries_only() {
        let t = Tmp::new("sweep");
        let home = home_at(&t.0, None);
        let layout = layout_at(&t.0);
        let luarc = t.0.join(".luarc.json");
        // A user entry, plus a managed entry from a version prova no longer serves.
        let stale = path_entry(&layout.annotations_dir().join("0.0.0-ancient"));
        std::fs::write(
            &luarc,
            serde_json::to_string(&json!({
                "diagnostics.globals": ["vim"],
                "workspace.library": ["types", stale],
            }))
            .unwrap(),
        )
        .unwrap();

        setup(&home, &BTreeMap::new(), Manage::Always, &layout, V).unwrap();
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&luarc).unwrap()).unwrap();
        let lib = doc["workspace.library"].as_array().unwrap();
        assert!(
            !lib.iter().any(|v| v.as_str() == Some(stale.as_str())),
            "stale managed entry survived: {lib:?}"
        );
        assert!(
            lib.iter().any(|v| v == "types"),
            "user entry was swept: {lib:?}"
        );
    }

    #[test]
    fn never_installs_stubs_but_leaves_luarc() {
        let t = Tmp::new("never");
        let home = home_at(&t.0, None);
        let layout = layout_at(&t.0);
        let out = setup(&home, &BTreeMap::new(), Manage::Never, &layout, V).unwrap();
        assert!(!out.luarc_created && !out.luarc_hint);
        assert!(!t.0.join(".luarc.json").exists());
        assert!(out.core_dir.join("prova.lua").is_file()); // stubs still installed
    }

    /// The core stubs install to a **stable, unversioned** path (so `.luarc.json` never churns on an
    /// upgrade) and carry a `.version` stamp. A version bump re-extracts and reclaims any stub we no
    /// longer ship, so the shared dir can't accumulate orphans across upgrades.
    #[test]
    fn core_stubs_install_stable_with_a_version_stamp() {
        let t = Tmp::new("stamp");
        let layout = layout_at(&t.0);

        let dir = install_core_stubs(&layout, "1.0.0").unwrap();
        // Stable path: the layout's annotations dir verbatim, no version segment.
        assert_eq!(dir, layout.annotations_dir());
        assert!(dir.join("prova.lua").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.join(".version")).unwrap(),
            "1.0.0"
        );

        // A stub from a prior version that we no longer ship lingers in the shared dir.
        let orphan = dir.join("removed_module.lua");
        std::fs::write(&orphan, "-- from an older prova").unwrap();

        // A version bump re-extracts: stamp advances, orphan reclaimed, current stubs intact.
        let dir2 = install_core_stubs(&layout, "2.0.0").unwrap();
        assert_eq!(dir2, dir);
        assert_eq!(
            std::fs::read_to_string(dir.join(".version")).unwrap(),
            "2.0.0"
        );
        assert!(
            !orphan.exists(),
            "a stub we no longer ship must be reclaimed on upgrade"
        );
        assert!(dir.join("prova.lua").is_file());
    }

    /// `setup` under `Always` is the force-create-or-merge path `prova init` / `prova ide setup`
    /// rely on: it creates when absent and merges idempotently when present (one core entry).
    #[test]
    fn always_creates_then_merges_idempotently() {
        let t = Tmp::new("init");
        let home = home_at(&t.0, None);
        let layout = layout_at(&t.0);
        let out = setup(&home, &BTreeMap::new(), Manage::Always, &layout, V).unwrap();
        assert!(out.luarc_created);
        let out2 = setup(&home, &BTreeMap::new(), Manage::Always, &layout, V).unwrap();
        assert!(!out2.luarc_created);
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(t.0.join(".luarc.json")).unwrap())
                .unwrap();
        let ours = path_entry(&out2.core_dir);
        let count = doc["workspace.library"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v.as_str() == Some(ours.as_str()))
            .count();
        assert_eq!(count, 1);
    }
}
