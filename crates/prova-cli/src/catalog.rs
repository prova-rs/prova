//! The `prova init` catalog — the set of archetypes prova can scaffold a package from.
//!
//! Prova ships a **built-in** catalog so `prova init` works with zero configuration, and
//! `~/.config/prova/config.toml` layers `[init.*]` entries on top of it:
//!
//! ```toml
//! [init.project]                  # a matching key REPLACES the built-in entry outright
//! description = "A standard prova package (a proof suite)"
//! source      = "https://github.com/prova-rs/prova-init-default-archetype.git#v1"
//! switches    = ["ci"]            # always passed to the render for this entry
//! defaults    = true              # take the archetype's default for any unanswered prompt
//!
//! [init.project.answers]          # baked answers — never prompted, always supplied
//! proof_dir = "proofs"
//!
//! [init.service]                  # a new key ADDS an entry
//! description = "A service package pre-wired for postgres + http"
//! source      = "/Users/me/archetypes/prova-service"
//! ```
//!
//! Replacement is whole-entry rather than field-level: redefining `project` means you own it, which
//! is easier to reason about than a half-inherited entry. A `source` is anything `prova-archetect`
//! resolves — a git URL (optionally `#ref`) or a local path.
//!
//! ## Package-state injection
//!
//! `init` tells every archetype where it is running — generically, not per-entry. When the current
//! directory is inside an existing package (manifest discovery walks up, like `prova` itself), the
//! render receives:
//!
//! - switch `prova:in-package`
//! - answer `prova_package_root` — the package root, relative to the cwd (`.` when they coincide)
//! - answer `prova_plugin_root` — the manifest's `[run] plugin_root`, when declared (package-root
//!   relative, verbatim)
//!
//! Outside a package none of these are supplied, so an archetype can distinguish the two by probing
//! `archetype.switches` / its context. Any archetype can use this state (an entry's own
//! `switches`/`answers` and the CLI still win on conflict); archetypes that don't care ignore it.
//! Whether an entry is *allowed* to render inside a package is the entry's `in_package` policy
//! (`deny` — the default, never-clobber — or `allow` for entries that augment a package).

use std::collections::BTreeMap;

use serde::Deserialize;

use prova_core::SystemLayout;

/// One catalog entry: an archetype plus how this key renders it.
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct InitEntry {
    /// One line, shown by `--list` and in the interactive select. This is what makes a key choosable.
    pub description: String,
    /// Git URL (optionally `#ref`) or local path.
    pub source: String,
    /// Archetype switches always passed for this entry; CLI `--switch` unions with these.
    #[serde(default)]
    pub switches: Vec<String>,
    /// Take the archetype's default for any prompt left unanswered, rather than prompting.
    #[serde(default)]
    pub defaults: bool,
    /// Baked answers — supplied to every render of this key, never prompted. CLI `--answer` wins.
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    /// Whether this entry may render inside an already-initialized package. `deny` (the default)
    /// keeps init's never-clobber guard: a manifest in the current directory is an error. `allow` is
    /// for entries that AUGMENT a package rather than create one (e.g. scaffolding a local plugin
    /// into `plugin_root`) — the guard steps aside and the archetype decides what to write, informed
    /// by the injected package state (see the module docs on state injection).
    #[serde(default)]
    pub in_package: InPackage,
}

/// The `in_package` policy for an entry. See [`InitEntry::in_package`].
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum InPackage {
    /// Refuse to render when the current directory already holds a manifest (never-clobber).
    #[default]
    Deny,
    /// Render even inside an initialized package — the entry augments it.
    Allow,
}

/// The merged catalog: built-in entries with the user's `[init.*]` layered over them.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub entries: BTreeMap<String, InitEntry>,
}

/// The shape of `~/.config/prova/config.toml`. Only `[init.*]` is claimed today; unknown tables are
/// ignored so the file can grow other sections (it is the future home of global defaults) without
/// this parser rejecting them.
#[derive(Debug, Deserialize, Default)]
struct UserConfig {
    #[serde(default)]
    init: BTreeMap<String, InitEntry>,
}

impl Catalog {
    /// The catalog prova ships with. `project` is present unconditionally, which is what makes
    /// `prova init` work on a machine with no config at all.
    pub fn builtin() -> Catalog {
        let mut entries = BTreeMap::new();
        entries.insert(
            "project".to_string(),
            InitEntry {
                description: "The full default prova package — a .prova/ nook (manifest, config, \
                              shared lib plugin) + a starter proof suite"
                    .to_string(),
                // Pinned to the released `v1` tag — reproducible scaffolding that doesn't drift when
                // the archetype's `main` moves.
                source: "https://github.com/prova-rs/prova-init-default-archetype.git#v1"
                    .to_string(),
                switches: Vec::new(),
                defaults: false,
                answers: BTreeMap::new(),
                in_package: InPackage::Deny,
            },
        );
        entries.insert(
            "plugin".to_string(),
            InitEntry {
                description: "A prova package that also exports a namespace — a plugin (init.lua + \
                              [plugin] + self-test)"
                    .to_string(),
                // Pinned to the released `v1` tag — reproducible scaffolding that doesn't drift when
                // the archetype's `main` moves.
                source: "https://github.com/prova-rs/prova-init-plugin-archetype.git#v1"
                    .to_string(),
                switches: Vec::new(),
                defaults: false,
                answers: BTreeMap::new(),
                // A plugin can be scaffolded INTO an existing package (the local variant) — the
                // archetype reads the injected package state and places itself under `plugin_root`.
                in_package: InPackage::Allow,
            },
        );
        Catalog { entries }
    }

    /// Load the built-in catalog and merge `<config_dir>/config.toml` over it. A missing config file
    /// is normal (most machines have none); an unreadable or malformed one is an error, because
    /// silently falling back to the built-ins would strand a user whose entries never appear.
    pub fn load(layout: &dyn SystemLayout) -> Result<Catalog, String> {
        let mut catalog = Catalog::builtin();
        let path = layout.config_dir().join("config.toml");
        if !path.is_file() {
            return Ok(catalog);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let user: UserConfig =
            toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
        // A user key replaces the built-in of the same name; a new key adds one.
        catalog.entries.extend(user.init);
        Ok(catalog)
    }

    /// Look up a key, or an error naming the keys that do exist — a typo should never render the
    /// wrong archetype or fail silently.
    pub fn get(&self, key: &str) -> Result<&InitEntry, String> {
        self.entries.get(key).ok_or_else(|| {
            // The built-in formerly named `default` — steer old muscle memory to the new key.
            let hint = if key == "default" && self.entries.contains_key("project") {
                " (the built-in default entry is now named \"project\")"
            } else {
                ""
            };
            format!(
                "unknown init key {key:?} — available: {}{hint}",
                self.keys_line()
            )
        })
    }

    /// The available keys, comma-separated, for error messages.
    pub fn keys_line(&self) -> String {
        self.entries.keys().cloned().collect::<Vec<_>>().join(", ")
    }

    /// `--list`: keys and descriptions, key-column aligned, on stdout so it pipes.
    pub fn print_list(&self) {
        let width = self.entries.keys().map(String::len).max().unwrap_or(0);
        for (key, entry) in &self.entries {
            println!("  {key:<width$}  {}", entry.description);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A layout whose config dir is a temp dir we control.
    struct At(PathBuf);
    impl SystemLayout for At {
        fn cache_dir(&self) -> PathBuf {
            self.0.join("cache")
        }
        fn data_dir(&self) -> PathBuf {
            self.0.join("data")
        }
        fn config_dir(&self) -> PathBuf {
            self.0.join("config")
        }
    }

    fn tmp(tag: &str) -> At {
        let d = std::env::temp_dir().join(format!("prova-catalog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("config")).unwrap();
        At(d)
    }

    fn write_config(at: &At, toml: &str) {
        std::fs::write(at.config_dir().join("config.toml"), toml).unwrap();
    }

    #[test]
    fn builtin_project_is_present_without_any_config() {
        let at = tmp("builtin");
        let c = Catalog::load(&at).unwrap();
        assert!(c.entries.contains_key("project"));
        assert!(!c.entries["project"].description.is_empty());
        std::fs::remove_dir_all(&at.0).ok();
    }

    #[test]
    fn user_key_adds_and_matching_key_replaces() {
        let at = tmp("merge");
        write_config(
            &at,
            "[init.project]\n\
             description = \"mine\"\n\
             source = \"/local/arch\"\n\
             switches = [\"ci\"]\n\
             defaults = true\n\
             in_package = \"allow\"\n\
             [init.project.answers]\n\
             proof_dir = \"tests\"\n\
             [init.service]\n\
             description = \"svc\"\n\
             source = \"/local/svc\"\n",
        );
        let c = Catalog::load(&at).unwrap();
        // Two builtins (project, plugin) with `project` replaced and `service` added → 3.
        assert_eq!(c.entries.len(), 3);
        let d = &c.entries["project"];
        assert_eq!(d.description, "mine");
        assert_eq!(d.source, "/local/arch"); // whole-entry replacement, not a field merge
        assert_eq!(d.switches, vec!["ci".to_string()]);
        assert!(d.defaults);
        assert_eq!(d.in_package, InPackage::Allow);
        assert_eq!(d.answers["proof_dir"], "tests");
        assert_eq!(c.entries["service"].description, "svc");
        assert!(c.entries.contains_key("plugin")); // the untouched builtin survives the merge
        assert_eq!(c.entries["service"].in_package, InPackage::Deny); // unstated → never-clobber
        std::fs::remove_dir_all(&at.0).ok();
    }

    #[test]
    fn malformed_config_names_the_file() {
        let at = tmp("bad");
        write_config(&at, "[init.broken\n");
        let err = Catalog::load(&at).unwrap_err();
        assert!(err.contains("config.toml"), "{err}");
        std::fs::remove_dir_all(&at.0).ok();
    }

    #[test]
    fn unknown_key_lists_the_available_ones() {
        let err = Catalog::builtin().get("bogus").unwrap_err();
        assert!(err.contains("bogus"), "{err}");
        assert!(err.contains("project"), "{err}");
    }

    #[test]
    fn the_old_default_key_points_at_project() {
        let err = Catalog::builtin().get("default").unwrap_err();
        assert!(err.contains("now named \"project\""), "{err}");
    }
}
