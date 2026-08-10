//! The `prova init` catalog — the set of archetypes prova can scaffold a package from.
//!
//! Prova ships a **built-in** catalog so `prova init` works with zero configuration, and
//! `~/.config/prova/config.toml` layers `[init.*]` entries on top of it:
//!
//! ```toml
//! [init.project]                  # a matching key REPLACES the built-in entry outright
//! description = "A standard prova package (a proof suite)"
//! source      = "https://github.com/prova-rs/prova-init-project-archetype.git#v2"
//! switches    = ["ci"]            # always passed to the render for this entry
//! defaults    = true              # take the archetype's default for any unanswered prompt
//!
//! [init.project.answers]          # baked answers — never prompted, always supplied
//! proof_dir = "proofs"
//!
//! [init.service]                  # a new key ADDS an entry
//! description = "A service package pre-wired for postgres + http"
//! source      = "/Users/me/archetypes/prova-service"
//!
//! [init.acme-api]                 # NO source — the key is looked up in the registries
//! ```
//!
//! Replacement is whole-entry rather than field-level: redefining `project` means you own it, which
//! is easier to reason about than a half-inherited entry. A `source` is anything `prova-archetect`
//! resolves — a git URL (optionally `#ref`) or a local path.
//!
//! ## Resolving a key — declare it, or look it up
//!
//! A key does not have to carry a URL, and prova does not derive one from the key's spelling. Keys
//! resolve down a four-rung ladder, first match wins:
//!
//! 1. **`[init.<key>]` with a `source`** — used verbatim. The escape hatch that needs no registry:
//!    a local path while authoring, a private URL, a fork.
//! 2. **`[init.<key>]` with no `source`** — the key is looked up across the configured registries
//!    (`archetypes/<key>.toml`), and the entry's `switches`/`answers`/`defaults` still apply. This is
//!    how you attach local policy to someone else's published archetype.
//! 3. **A built-in key** (`project`, `package`) — prova's own, carrying explicit pinned sources so
//!    `prova init project` works on a machine with no config and no registry reachable.
//! 4. **A bare registry name** — never declared anywhere. `prova init acme-api` renders whatever the
//!    registries serve under that name, zero config.
//!
//! Two ordering choices worth stating, because both are the surprising-behaviour-avoidance kind:
//!
//! - **A built-in beats a registry entry of the same name.** Publishing an archetype called `project`
//!   to a registry must not silently change what `prova init project` does on every machine that
//!   lists it. Overriding a built-in is deliberate: one line of config, local and auditable (rung 1).
//! - **`--list` shows the catalog, not the registries.** It stays offline, fast, and predictable —
//!   the curated set you can render right now. The open namespace is reached by naming a key
//!   (rung 4), mirroring how `[dependencies]` is the committed set while `prova packages` browses.
//!
//! ## Package-state injection
//!
//! `init` tells every archetype where it is running — generically, not per-entry. When the current
//! directory is inside an existing package (manifest discovery walks up, like `prova` itself), the
//! render receives:
//!
//! - switch `prova:in-package`
//! - answer `prova_package_root` — the package root, relative to the cwd (`.` when they coincide)
//! - answer `prova_packages_dir` (and its deprecated `prova_plugin_root` alias) — the manifest's `[run] packages` directory, when declared (package-root
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
    /// Optional for a lookup entry — a key declared with no `source` takes its description from the
    /// registry that serves it, so an org publishes the text once instead of in every config.
    #[serde(default)]
    pub description: String,
    /// Git URL (optionally `#ref`) or local path. **Omit it to look the key up in the configured
    /// registries** — the indirection that lets an archetype live anywhere without prova (or the
    /// user's config) knowing its URL, and without the key having to encode a repo naming pattern.
    #[serde(default)]
    pub source: Option<String>,
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
    /// for entries that AUGMENT a package rather than create one (e.g. scaffolding a local package
    /// into the `packages` directory) — the guard steps aside and the archetype decides what to write, informed
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
                              shared lib package) + a starter proof suite"
                    .to_string(),
                // Pinned to the released `v1` tag — reproducible scaffolding that doesn't drift when
                // the archetype's `main` moves.
                source: Some(
                    "https://github.com/prova-rs/prova-init-project-archetype.git#v2".to_string(),
                ),
                switches: Vec::new(),
                defaults: false,
                answers: BTreeMap::new(),
                in_package: InPackage::Deny,
            },
        );
        entries.insert(
            "package".to_string(),
            InitEntry {
                description: "A prova package that also exports a namespace others require() \
                              (init.lua + [package] + self-test)"
                    .to_string(),
                // Pinned to the released `v1` tag — reproducible scaffolding that doesn't drift when
                // the archetype's `main` moves. (The repo is being renamed prova-init-package-\
                // archetype; GitHub redirects the old name until the pin moves.)
                source: Some(
                    "https://github.com/prova-rs/prova-init-package-archetype.git#v2".to_string(),
                ),
                switches: Vec::new(),
                defaults: false,
                answers: BTreeMap::new(),
                // A package can be scaffolded INTO an existing one (the local variant) — the
                // archetype reads the injected package state and places itself under the `packages`
                // directory.
                in_package: InPackage::Allow,
            },
        );
        Catalog { entries }
    }

    /// Deprecated entry spellings — the catalog side of the package-vocabulary bridge. Resolution
    /// still lands on the canonical entry; the warning teaches the rename. Retires at 1.0.
    pub const DEPRECATED_ENTRIES: &'static [(&'static str, &'static str)] =
        &[("plugin", "package")];

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

    /// The available keys, comma-separated, for error messages.
    pub fn keys_line(&self) -> String {
        self.entries.keys().cloned().collect::<Vec<_>>().join(", ")
    }

    /// `--list`: keys and descriptions, key-column aligned, on stdout so it pipes.
    ///
    /// Deliberately the catalog only — no registry fetch. This is the "what can I render right now"
    /// answer and it must stay fast and offline; the open namespace is reached by naming a key. The
    /// trailing note is how a reader learns that, since an empty-looking list would otherwise imply
    /// those are the only options.
    pub fn print_list(&self) {
        let width = self.entries.keys().map(String::len).max().unwrap_or(0);
        for (key, entry) in &self.entries {
            let description = if entry.description.is_empty() {
                "(from the registry that serves this key)"
            } else {
                &entry.description
            };
            println!("  {key:<width$}  {description}");
        }
        println!();
        println!("  Any archetype a configured registry serves can also be named directly");
        println!("  (`prova init <name>`), whether or not it is listed above.");
    }
}

/// A key resolved to something renderable: where the archetype comes from, plus the policy that
/// applies to this render. Built from a catalog entry, a registry lookup, or both.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// What `prova-archetect` will render — a git URL (optionally `#ref`) or a local path.
    pub source: String,
    /// Where the source came from, for the render announcement: the catalog, or a named registry.
    pub origin: String,
    /// One line describing what this archetype produces. For a registry-resolved key this is the
    /// publisher's own text — the only description a user naming an undeclared key has ever seen, so
    /// the render announces it rather than rendering something unexplained.
    pub description: String,
    pub switches: Vec<String>,
    pub defaults: bool,
    pub answers: BTreeMap<String, String>,
    pub in_package: InPackage,
}

/// Say something when a registry-sourced render will not be reproducible. Both cases are the entry's
/// doing, not the user's, so the user is told rather than left to discover it from a drifting render.
fn note_pinning(key: &str, found: &crate::registry::ArchetypeEntry, warnings: &mut Vec<String>) {
    if found.latest_ignored() {
        warnings.push(format!(
            "archetype {key:?} recommends `latest` but its repo is a local path — rendering the \
             directory as-is, unpinned"
        ));
    } else if found.latest.is_none() {
        warnings.push(format!(
            "archetype {key:?} recommends no `latest` — rendering its default branch, unpinned"
        ));
    }
}

/// Resolve `key` down the ladder documented at the top of this module: a declared `source` wins, then
/// a declared key looked up in the registries, then a built-in, then a bare registry name.
///
/// Registry access is lazy — a key with a declared `source` never touches the network, which is what
/// keeps `prova init project` a single fetch on a fresh machine. `warnings` collects registry
/// tolerance messages (a skipped entry, an unreachable registry, a shadowed key) for the caller to
/// print: none of them should sink a render that the remaining registries can serve.
pub fn resolve(
    catalog: &Catalog,
    key: &str,
    layout: &dyn SystemLayout,
    warnings: &mut Vec<String>,
) -> Result<Resolved, String> {
    // Rung 0: a deprecated spelling of a built-in key — resolve the canonical entry, teach the
    // rename. A user config that deliberately declares the old key wins (checked first below).
    let key = if catalog.entries.contains_key(key) {
        key
    } else if let Some((_, new)) = Catalog::DEPRECATED_ENTRIES.iter().find(|(old, _)| *old == key)
    {
        eprintln!("prova: `prova init {key}` is deprecated — use `prova init {new}` (retires at 1.0)");
        new
    } else {
        key
    };
    // Rungs 1–3: the key is in the catalog (a config entry has already replaced any built-in of the
    // same name during `load`, so this one lookup covers both).
    if let Some(entry) = catalog.entries.get(key) {
        let (source, origin, description) = match &entry.source {
            Some(s) => (s.clone(), "the catalog".to_string(), entry.description.clone()),
            None => {
                // Rung 2: declared for its policy, sourced from a registry.
                let found = crate::registry::lookup_archetype(key, layout, warnings)?
                    .ok_or_else(|| {
                        format!(
                            "init key {key:?} declares no `source` and no configured registry \
                             serves an archetype by that name — add a `source`, or check the \
                             registries in config.toml"
                        )
                    })?;
                let origin = format!("registry {}", found.registry);
                // A locally-declared description wins: the config author may well have a better name
                // for it in their context than the publisher does.
                let description = if entry.description.is_empty() {
                    found.description.clone()
                } else {
                    entry.description.clone()
                };
                // Same pinning caveats as rung 4 — declaring the key locally changes the policy that
                // applies, not where the archetype comes from or how reproducible the render is.
                note_pinning(key, &found, warnings);
                (found.source(), origin, description)
            }
        };
        return Ok(Resolved {
            source,
            origin,
            description,
            switches: entry.switches.clone(),
            defaults: entry.defaults,
            answers: entry.answers.clone(),
            in_package: entry.in_package,
        });
    }

    // Rung 4: not declared anywhere — the open namespace. A registry hit carries its own
    // `in_package` policy, because only the archetype knows whether it creates or augments.
    if let Some(found) = crate::registry::lookup_archetype(key, layout, warnings)? {
        let in_package = match found.in_package.as_deref() {
            Some("allow") => InPackage::Allow,
            Some("deny") | None => InPackage::Deny,
            Some(other) => {
                warnings.push(format!(
                    "archetype {key:?}: unknown in_package {other:?} — treating as \"deny\""
                ));
                InPackage::Deny
            }
        };
        note_pinning(key, &found, warnings);
        let origin = format!("registry {}", found.registry);
        return Ok(Resolved {
            source: found.source(),
            origin,
            description: found.description.clone(),
            switches: Vec::new(),
            defaults: false,
            answers: BTreeMap::new(),
            in_package,
        });
    }

    // Nothing anywhere. Keep the catalog keys in the message — a typo is the common case — and say
    // that the registries were consulted, so "not found" is not mistaken for "not looked for".
    let hint = if key == "default" && catalog.entries.contains_key("project") {
        " (the built-in default entry is now named \"project\")"
    } else {
        ""
    };
    Err(format!(
        "unknown init key {key:?} — available: {}{hint}; no configured registry serves an \
         archetype by that name either",
        catalog.keys_line()
    ))
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

    /// Every built-in must be renderable with **no config and no registry** — that is the whole point
    /// of shipping built-ins — and pinned so scaffolding does not drift when a `main` branch moves.
    ///
    /// This replaced an assertion that each built-in's URL matched `prova-rs/prova-init-<key>-archetype`.
    /// That version caught a real bug (the `project` key was renamed from `default`, but its URL kept
    /// naming `prova-init-default-archetype` and survived only on a GitHub redirect) — but it caught it
    /// by encoding a repo-naming convention as a rule, which is exactly wrong now that a key can name
    /// an archetype anyone hosts anywhere. Keys do not imply URLs; they resolve. What matters is that
    /// the shipped defaults are self-contained and pinned, which is what this asserts.
    #[test]
    fn builtins_are_self_contained_and_pinned() {
        for (key, entry) in &Catalog::builtin().entries {
            let source = entry
                .source
                .as_ref()
                .unwrap_or_else(|| panic!("built-in {key:?} must carry a source, not a lookup"));
            assert!(
                source.contains('#'),
                "built-in {key:?} must be pinned to a ref, got {source:?}"
            );
            assert!(
                !entry.description.is_empty(),
                "built-in {key:?} must describe itself — it is what --list shows"
            );
        }
    }

    /// A local-path registry serving one archetype, plus a config that REPLACES the built-in
    /// `prova-rs` registry with it.
    ///
    /// Replacing rather than adding is what keeps these tests hermetic: merge-by-name drops the real
    /// `prova-rs/package-registry` out of the set, so nothing here can reach the network. Same trick
    /// `proofs/spec/registry/registry_test.lua` uses.
    fn with_local_registry(at: &At, extra_init: &str) {
        let reg = at.0.join("registry-repo");
        std::fs::create_dir_all(reg.join("archetypes")).unwrap();
        std::fs::write(
            reg.join("archetypes").join("acme-api.toml"),
            "schema = 1\n\
             name = \"acme-api\"\n\
             repo = \"https://git.acme.internal/archetypes/api\"\n\
             description = \"An Acme API package\"\n\
             latest = \"v3\"\n\
             in_package = \"deny\"\n",
        )
        .unwrap();
        // An archetype whose key collides with a built-in — the shadowing case.
        std::fs::write(
            reg.join("archetypes").join("project.toml"),
            "schema = 1\n\
             name = \"project\"\n\
             repo = \"https://git.acme.internal/archetypes/project\"\n\
             description = \"Acme's idea of a project\"\n\
             latest = \"v9\"\n",
        )
        .unwrap();
        write_config(
            at,
            &format!(
                // A TOML *literal* string (single quotes): a Windows path is `C:\…\registry-repo`,
                // and in a basic (double-quoted) string those backslashes are escape sequences, so
                // `\U`/`\r`/… make the config unparseable. Literal strings take the path verbatim.
                "[[registries]]\n\
                 name = \"prova-rs\"\n\
                 source = '{}'\n{extra_init}",
                reg.display()
            ),
        );
    }

    /// Rung 4: a key nobody declared, served by a registry. Zero config beyond listing the registry —
    /// this is the case that makes third-party archetypes usable without prova knowing their URLs.
    #[test]
    fn an_undeclared_key_resolves_through_the_registry() {
        let at = tmp("lookup-bare");
        with_local_registry(&at, "");
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let r = resolve(&c, "acme-api", &at, &mut warnings).expect("registry lookup");

        // `latest` becomes the pin, so a bare key still renders something reproducible — and the
        // browser-shaped registry repo gains the `.git` suffix archetect's source detection needs.
        assert_eq!(r.source, "https://git.acme.internal/archetypes/api.git#v3");
        assert_eq!(r.origin, "registry prova-rs");
        assert_eq!(r.description, "An Acme API package");
        assert_eq!(r.in_package, InPackage::Deny);
        std::fs::remove_dir_all(&at.0).ok();
    }

    /// Rung 2: declared for its POLICY, sourced from the registry. The point of the split — an org
    /// attaches its own switches/answers to someone else's published archetype without pasting a URL.
    #[test]
    fn a_declared_key_without_a_source_takes_the_registry_repo_and_keeps_local_policy() {
        let at = tmp("lookup-policy");
        with_local_registry(
            &at,
            "[init.acme-api]\n\
             switches = [\"ci\"]\n\
             defaults = true\n\
             [init.acme-api.answers]\n\
             team = \"platform\"\n",
        );
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let r = resolve(&c, "acme-api", &at, &mut warnings).expect("lookup with policy");

        assert_eq!(r.source, "https://git.acme.internal/archetypes/api.git#v3");
        assert_eq!(r.switches, vec!["ci".to_string()]);
        assert!(r.defaults);
        assert_eq!(r.answers.get("team").map(String::as_str), Some("platform"));
        // No local description declared, so the publisher's is used.
        assert_eq!(r.description, "An Acme API package");
        std::fs::remove_dir_all(&at.0).ok();
    }

    /// A built-in beats a registry entry of the same name. Publishing an archetype called `project`
    /// must not silently change what `prova init project` does on every machine listing that registry.
    #[test]
    fn a_builtin_key_is_not_shadowed_by_a_registry_entry() {
        let at = tmp("no-shadow");
        with_local_registry(&at, "");
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let r = resolve(&c, "project", &at, &mut warnings).expect("built-in project");

        assert!(
            r.source.contains("prova-init-project-archetype"),
            "the built-in must win, got {:?}",
            r.source
        );
        assert_eq!(r.origin, "the catalog");
        std::fs::remove_dir_all(&at.0).ok();
    }

    /// ...and overriding it IS possible — explicitly, with one local line. That is the sanctioned way
    /// to have a different idea of what "project" means.
    #[test]
    fn an_explicit_source_overrides_a_builtin_key() {
        let at = tmp("override");
        with_local_registry(
            &at,
            "[init.project]\n\
             description = \"Acme project\"\n\
             source = \"/acme/archetypes/project\"\n",
        );
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let r = resolve(&c, "project", &at, &mut warnings).expect("overridden project");

        assert_eq!(r.source, "/acme/archetypes/project");
        assert_eq!(r.description, "Acme project");
        std::fs::remove_dir_all(&at.0).ok();
    }

    /// An unknown key must say both things: what the catalog offers, AND that the registries were
    /// consulted — otherwise "not found" reads as "prova never looked".
    #[test]
    fn an_unknown_key_names_the_catalog_and_says_registries_were_searched() {
        let at = tmp("unknown-key");
        with_local_registry(&at, "");
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let err = resolve(&c, "nope", &at, &mut warnings).expect_err("unknown key must fail");

        assert!(err.contains("project"), "names the catalog keys: {err}");
        assert!(err.contains("registry"), "says registries were searched: {err}");
        std::fs::remove_dir_all(&at.0).ok();
    }

    /// A declared key with no source and no registry serving it must say which of the two to fix,
    /// rather than reporting a bare "unknown key" for a key the user can plainly see in their config.
    #[test]
    fn a_lookup_key_with_no_registry_hit_explains_the_two_fixes() {
        let at = tmp("lookup-miss");
        with_local_registry(&at, "[init.ghost]\n");
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let err = resolve(&c, "ghost", &at, &mut warnings).expect_err("no registry serves ghost");

        assert!(err.contains("declares no `source`"), "{err}");
        assert!(err.contains("registries in config.toml"), "{err}");
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
        assert_eq!(d.source.as_deref(), Some("/local/arch")); // whole-entry, not a field merge
        assert_eq!(d.switches, vec!["ci".to_string()]);
        assert!(d.defaults);
        assert_eq!(d.in_package, InPackage::Allow);
        assert_eq!(d.answers["proof_dir"], "tests");
        assert_eq!(c.entries["service"].description, "svc");
        assert!(c.entries.contains_key("package")); // the untouched builtin survives the merge
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

    /// The built-in formerly named `default` — steer old muscle memory to the new key. Resolved
    /// against a hermetic local registry so the miss is decided offline, not by a network fetch.
    #[test]
    fn the_old_default_key_points_at_project() {
        let at = tmp("default-hint");
        with_local_registry(&at, "");
        let c = Catalog::load(&at).unwrap();
        let mut warnings = Vec::new();
        let err = resolve(&c, "default", &at, &mut warnings).expect_err("`default` is not a key");
        assert!(err.contains("now named \"project\""), "{err}");
        std::fs::remove_dir_all(&at.0).ok();
    }
}
