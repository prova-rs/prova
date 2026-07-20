//! The `prova init` catalog — the set of archetypes prova can scaffold a project from.
//!
//! Prova ships a **built-in** catalog so `prova init` works with zero configuration, and
//! `~/.config/prova/config.toml` layers `[init.*]` entries on top of it:
//!
//! ```toml
//! [init.default]                  # a matching key REPLACES the built-in entry outright
//! description = "Standard prova proof suite"
//! source      = "https://github.com/prova-rs/prova-init-default-archetype.git#main"
//! switches    = ["ci"]            # always passed to the render for this entry
//! defaults    = true              # take the archetype's default for any unanswered prompt
//!
//! [init.default.answers]          # baked answers — never prompted, always supplied
//! proof_dir = "proofs"
//!
//! [init.service]                  # a new key ADDS an entry
//! description = "A service proof suite pre-wired for postgres + http"
//! source      = "/Users/me/archetypes/prova-service"
//! ```
//!
//! Replacement is whole-entry rather than field-level: redefining `default` means you own it, which
//! is easier to reason about than a half-inherited entry. A `source` is anything `prova-archetect`
//! resolves — a git URL (optionally `#ref`) or a local path.

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
    /// The catalog prova ships with. `default` is present unconditionally, which is what makes
    /// `prova init` work on a machine with no config at all.
    pub fn builtin() -> Catalog {
        let mut entries = BTreeMap::new();
        entries.insert(
            "default".to_string(),
            InitEntry {
                description: "Standard prova proof suite (prova.toml + a first proof)".to_string(),
                // Tracks the archetype's `main` while it stabilizes; pin to `#v1` once that tag is cut.
                source: "https://github.com/prova-rs/prova-init-default-archetype.git#main"
                    .to_string(),
                switches: Vec::new(),
                defaults: false,
                answers: BTreeMap::new(),
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
        let user: UserConfig = toml::from_str(&text)
            .map_err(|e| format!("invalid {}: {e}", path.display()))?;
        // A user key replaces the built-in of the same name; a new key adds one.
        catalog.entries.extend(user.init);
        Ok(catalog)
    }

    /// Look up a key, or an error naming the keys that do exist — a typo should never render the
    /// wrong archetype or fail silently.
    pub fn get(&self, key: &str) -> Result<&InitEntry, String> {
        self.entries.get(key).ok_or_else(|| {
            format!(
                "unknown init key {key:?} — available: {}",
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
    fn builtin_default_is_present_without_any_config() {
        let at = tmp("builtin");
        let c = Catalog::load(&at).unwrap();
        assert!(c.entries.contains_key("default"));
        assert!(!c.entries["default"].description.is_empty());
        std::fs::remove_dir_all(&at.0).ok();
    }

    #[test]
    fn user_key_adds_and_matching_key_replaces() {
        let at = tmp("merge");
        write_config(
            &at,
            "[init.default]\n\
             description = \"mine\"\n\
             source = \"/local/arch\"\n\
             switches = [\"ci\"]\n\
             defaults = true\n\
             [init.default.answers]\n\
             proof_dir = \"tests\"\n\
             [init.service]\n\
             description = \"svc\"\n\
             source = \"/local/svc\"\n",
        );
        let c = Catalog::load(&at).unwrap();
        assert_eq!(c.entries.len(), 2);
        let d = &c.entries["default"];
        assert_eq!(d.description, "mine");
        assert_eq!(d.source, "/local/arch"); // whole-entry replacement, not a field merge
        assert_eq!(d.switches, vec!["ci".to_string()]);
        assert!(d.defaults);
        assert_eq!(d.answers["proof_dir"], "tests");
        assert_eq!(c.entries["service"].description, "svc");
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
        assert!(err.contains("default"), "{err}");
    }
}
