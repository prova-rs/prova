//! The suite manifest (`prova.toml`) — declare *what* to run and *how*, so `prova` with no
//! arguments runs the configured suite and CI is just `prova`. A general-purpose harness needs a
//! stable way to name suites and environments across local and CI; this is it.
//!
//! ```toml
//! [run]                         # the default profile
//! paths  = ["tests"]            # files/dirs to discover
//! jobs   = 4                    # concurrency (throughput only)
//! format = "console"           # "console" | "json"
//!
//! [run.env]                     # environment for the run
//! LOG = "info"
//!
//! [profiles.ci]                 # `prova --profile ci` overlays this on [run]
//! jobs = 8
//! must_run = ["docker", "dotnet >= 9"]   # capabilities CI GUARANTEES: unmet → fail, never skip.
//!                               # (A test's `requires` says what it needs; a profile's `must_run`
//!                               #  says what this environment promises. Same expression grammar,
//!                               #  and the reason a suite whose every test skipped can't exit 0
//!                               #  here. A version constraint is the difference between "dotnet is
//!                               #  installed" and "dotnet can build this".)
//! [profiles.ci.env]
//! CI = "true"
//! [profiles.ci.plugins]         # CI-only capabilities, still pinned in-repo (not an out-of-band input)
//! toxiproxy = { git = "https://github.com/acme/prova-toxiproxy", tag = "v1" }
//!
//! ```
//!
//! An optional `prova.lua` beside this file is the project's Lua companion (the pairing archetect
//! uses for archetype.yaml + archetype.lua). It is where `runtime.capability(name, fn)` registers a
//! project-wide predicate — a GPU, a kind cluster — for a capability no name-and-version can
//! express. It loads WITH the manifest, which is what lets `must_run` guarantee one: the
//! precondition is checked before any suite exists, so a suite-registered capability would not yet.
//!
//! ```toml
//! [suites.grpc]                 # an explicit suite: these files share one state (Scope.Suite)
//! paths = ["services/grpc"]     # (a directory's own `suite.lua` is the zero-config alternative)
//! setup = "services/grpc/suite.lua"
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;

/// A parsed `prova.toml`. The `[run]` table is the default profile; `[profiles.<name>]` tables are
/// overlays selected with `--profile <name>`. `[suites.<name>]` tables declare explicit suites for
/// grouping that doesn't match the directory tree (a directory's `suite.lua` is the zero-config path).
#[derive(Debug, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub run: Profile,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub suites: BTreeMap<String, SuiteDecl>,
    /// Declared plugins: `require(name)` resolves to this source (a local file/dir or a git repo).
    /// Not profile-specific — the plugin set is a property of the project, applied to every run.
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginSource>,
    /// Registered source aliases for plugin shorthands: `alias → base`, where `base` is a host
    /// shorthand (`github:acme`) or a base URL (`https://github.com/acme`). A plugin written
    /// `"acme:redis"` then expands via `acme` to `https://github.com/acme/redis`.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// How prova manages the project's LuaLS IDE integration (`.luarc.json` + synced annotations).
    /// Not profile-specific — a property of the project.
    #[serde(default)]
    pub luals: Luals,
}

/// LuaLS / `.luarc.json` management policy. The annotation set under `<home>/annotations/` is always
/// refreshed (it's prova-owned and gitignored); this only governs whether prova writes the *pointer*
/// (`.luarc.json` at the project root).
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
pub struct Luals {
    /// `"auto"` (default) | `"always"` | `"never"`. See [`Manage`].
    pub manage: Option<String>,
}

/// Resolved `.luarc.json` management policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manage {
    /// Create `.luarc.json` if absent; if one already exists, don't edit it — print a hint to run
    /// `prova init`. This auto-detects project type: a non-Lua project (Lua present only for prova)
    /// has no `.luarc.json`, so prova sets it up; a Lua-native project already owns one, so prova
    /// stays a polite guest.
    Auto,
    /// Always create-or-merge our two keys into `.luarc.json`, even into an existing file.
    Always,
    /// Never touch `.luarc.json` (annotations still sync; the user wires the pointer themselves).
    Never,
}

impl Manage {
    /// Parse a `manage` value (`[luals] manage` or `--manage`), defaulting to `Auto` when absent. An
    /// unrecognized value is an error the caller surfaces.
    pub fn parse(value: Option<&str>) -> Result<Manage, String> {
        match value {
            None | Some("auto") => Ok(Manage::Auto),
            Some("always") => Ok(Manage::Always),
            Some("never") => Ok(Manage::Never),
            Some(other) => Err(format!(
                "invalid manage = {other:?} (expected \"auto\", \"always\", or \"never\")"
            )),
        }
    }
}

impl Luals {
    /// Resolve the policy, defaulting to `Auto`. An unrecognized value is an error the caller surfaces.
    pub fn manage(&self) -> Result<Manage, String> {
        Manage::parse(self.manage.as_deref())
    }
}

/// Where a declared plugin's Lua comes from. The string shorthand is a local path; the table form
/// adds git and an in-repo `module` path.
///
/// ```toml
/// [plugins]
/// greet    = "./plugins/greet.lua"                                   # local path shorthand
/// fixtures = { path = "./test-support" }                             # local dir (fixtures.lua / init.lua)
/// rabbitmq = { git = "https://github.com/acme/prova-rabbitmq", tag = "v1.0.0" }
/// nats     = { git = "https://github.com/acme/prova-nats", rev = "abc123", module = "src/nats.lua" }
/// ```
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum PluginSource {
    /// A local path to a `.lua` file or a directory (resolved to `<name>.lua` then `init.lua`).
    Path(String),
    /// The detailed form: a local `path` or a `git` repo, with an optional in-repo `module` path and
    /// a pin (`tag` / `branch` / `rev`).
    Detailed(PluginDetail),
}

/// The table form of a plugin source. Exactly one of `path` / `git` is expected.
#[derive(Debug, Deserialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct PluginDetail {
    /// A local path to a `.lua` file or a directory.
    pub path: Option<String>,
    /// A git repository URL.
    pub git: Option<String>,
    /// Pin to a tag (shallow-cloned).
    pub tag: Option<String>,
    /// Pin to a branch (shallow-cloned).
    pub branch: Option<String>,
    /// Pin to a specific commit (cloned, then checked out).
    pub rev: Option<String>,
    /// Path within the repo/dir to the module file; defaults to `<name>.lua` then `init.lua`.
    pub module: Option<String>,
}

/// An explicitly-declared suite: its `paths` are discovered into one suite (sharing an optional
/// `setup` `suite.lua`). Requires/env belong in the setup file (`suite.config`) / `[run.env]`.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct SuiteDecl {
    #[serde(default)]
    pub paths: Vec<String>,
    pub setup: Option<String>,
}

/// One run profile. Every field is optional so a profile can override just what it needs.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Profile {
    #[serde(default)]
    pub paths: Vec<String>,
    /// The companion file loaded once, pre-suite, for `runtime.*` config (capabilities). Relative to
    /// the home. Defaults to `prova.lua` beside the manifest; point it elsewhere (e.g.
    /// `proofs/shared/config.lua`) to keep the home clean.
    pub config: Option<String>,
    pub jobs: Option<usize>,
    pub format: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Profile-scoped plugins (`[profiles.<name>.plugins]`), overlaid on the project-wide
    /// `[plugins]` set. The principled home for CI-only capabilities: declared in `prova.toml` so a
    /// `--profile ci` run and local dev resolve the same pinned source, instead of injecting plugins
    /// through an out-of-band CI input. On a name conflict the profile's entry wins.
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginSource>,
    /// Capabilities this context **guarantees** — checked as a precondition, before anything runs.
    ///
    /// The other half of `requires`, and the reason they are two things: a test's `requires` is a
    /// portable *fact about the test* ("I need docker"), true on a laptop and in CI alike. A
    /// profile's `must_run` is *policy about the environment* ("here, docker is promised"), and it
    /// changes when you move without the test changing at all. That is the same seam the port modes
    /// use — the definition is decoupled from the verb.
    ///
    /// A guaranteed capability that is absent is a **broken environment, not a skipped test**, so it
    /// fails the run. Without this, a suite whose every test skips exits 0, and "we answered
    /// everything" is indistinguishable from "we could not ask anything" (docs/design/test-topology.md).
    ///
    /// Generic over the whole capability vocabulary — the same names and probes `requires` uses, so
    /// `must_run = ["kind"]` needs no new detector.
    #[serde(default)]
    pub must_run: Vec<String>,
}

/// A fully-resolved run configuration (base `[run]` with an optional profile overlaid).
#[derive(Debug, PartialEq)]
pub struct Resolved {
    pub paths: Vec<String>,
    /// The companion config file (relative to home); `None` → the `prova.lua` default. See `Profile`.
    pub config: Option<String>,
    pub jobs: Option<usize>,
    pub format: Option<String>,
    pub env: BTreeMap<String, String>,
    /// Explicitly-declared suites (`[suites.*]`), run in addition to `paths`.
    pub suites: BTreeMap<String, SuiteDecl>,
    /// Declared plugins (`[plugins.*]`) — name → source, applied to every run.
    pub plugins: BTreeMap<String, PluginSource>,
    /// Registered source aliases (`[sources]`) for plugin shorthands.
    pub sources: BTreeMap<String, String>,
    /// LuaLS IDE-integration policy (`[luals]`).
    pub luals: Luals,
    /// Capabilities this run guarantees — the union of `[run] must_run` and the selected profile's.
    /// A guarantee is **additive**: a profile promises *more* than the project baseline, never less,
    /// because a context that could retract a guarantee would let the strictest bar be silenced by
    /// selecting a laxer profile.
    pub must_run: Vec<String>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        toml::from_str(text).map_err(|e| format!("invalid prova.toml: {e}"))
    }

    /// Overlay a profile on the base `[run]` profile. `None` uses the base as-is; `Some(name)` takes
    /// each field from the profile when present, otherwise from the base. Env is base-then-profile
    /// (profile wins). Errors if the named profile does not exist.
    pub fn resolve(&self, profile: Option<&str>) -> Result<Resolved, String> {
        let base = &self.run;
        let overlay = match profile {
            None => None,
            Some(name) => Some(
                self.profiles
                    .get(name)
                    .ok_or_else(|| format!("no such profile {name:?} in prova.toml"))?,
            ),
        };

        let paths = match overlay {
            Some(p) if !p.paths.is_empty() => p.paths.clone(),
            _ => base.paths.clone(),
        };
        let config = overlay
            .and_then(|p| p.config.clone())
            .or_else(|| base.config.clone());
        let jobs = overlay.and_then(|p| p.jobs).or(base.jobs);
        let format = overlay
            .and_then(|p| p.format.clone())
            .or_else(|| base.format.clone());

        let mut env = base.env.clone();
        if let Some(p) = overlay {
            for (k, v) in &p.env {
                env.insert(k.clone(), v.clone());
            }
        }

        // Project-wide `[plugins]` are the base; the selected profile's `[profiles.X.plugins]`
        // overlay it (profile wins on a name conflict), so a CI profile can add capabilities without
        // an out-of-band input and local `--profile ci` resolves identically.
        let mut plugins = self.plugins.clone();
        if let Some(p) = overlay {
            for (k, v) in &p.plugins {
                plugins.insert(k.clone(), v.clone());
            }
        }

        // Guarantees are the UNION of the baseline and the profile's — additive, never overriding,
        // unlike `paths`/`jobs`/`format`. A profile promises more than the project, never less.
        let mut must_run = base.must_run.clone();
        if let Some(p) = overlay {
            for cap in &p.must_run {
                if !must_run.contains(cap) {
                    must_run.push(cap.clone());
                }
            }
        }

        Ok(Resolved {
            paths,
            config,
            jobs,
            format,
            env,
            suites: self.suites.clone(),
            plugins,
            sources: self.sources.clone(),
            luals: self.luals.clone(),
            must_run,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[run]
paths  = ["tests"]
jobs   = 4
format = "console"

[run.env]
LOG = "info"

[profiles.ci]
jobs = 8
[profiles.ci.env]
CI = "true"

[profiles.smoke]
paths = ["tests/smoke"]
"#;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn default_profile_uses_run_table() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let r = m.resolve(None).unwrap();
        assert_eq!(
            r,
            Resolved {
                paths: vec!["tests".into()],
                config: None,
                jobs: Some(4),
                format: Some("console".into()),
                env: env(&[("LOG", "info")]),
                suites: BTreeMap::new(),
                plugins: BTreeMap::new(),
                sources: BTreeMap::new(),
                luals: Luals::default(),
                must_run: Vec::new(),
            }
        );
    }

    #[test]
    fn parses_plugin_sources_in_both_forms() {
        let m = Manifest::parse(
            r#"
[run]
paths = ["tests"]

[plugins]
greet    = "./plugins/greet.lua"
fixtures = { path = "./test-support" }
rabbitmq = { git = "https://example.com/acme/prova-rabbitmq", tag = "v1.0.0" }
nats     = { git = "https://example.com/acme/prova-nats", rev = "abc123", module = "src/nats.lua" }
"#,
        )
        .unwrap();
        let r = m.resolve(None).unwrap();
        assert_eq!(r.plugins.len(), 4);
        assert_eq!(
            r.plugins["greet"],
            PluginSource::Path("./plugins/greet.lua".into())
        );
        assert_eq!(
            r.plugins["fixtures"],
            PluginSource::Detailed(PluginDetail {
                path: Some("./test-support".into()),
                ..Default::default()
            })
        );
        assert_eq!(
            r.plugins["rabbitmq"],
            PluginSource::Detailed(PluginDetail {
                git: Some("https://example.com/acme/prova-rabbitmq".into()),
                tag: Some("v1.0.0".into()),
                ..Default::default()
            })
        );
        assert_eq!(
            r.plugins["nats"],
            PluginSource::Detailed(PluginDetail {
                git: Some("https://example.com/acme/prova-nats".into()),
                rev: Some("abc123".into()),
                module: Some("src/nats.lua".into()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn declares_explicit_suites() {
        let m = Manifest::parse(
            r#"
[run]
paths = ["tests"]

[suites.grpc]
paths = ["services/grpc"]
setup = "services/grpc/suite.lua"

[suites.rest]
paths = ["services/rest"]
"#,
        )
        .unwrap();
        let r = m.resolve(None).unwrap();
        assert_eq!(r.suites.len(), 2);
        assert_eq!(r.suites["grpc"].paths, vec!["services/grpc".to_string()]);
        assert_eq!(
            r.suites["grpc"].setup.as_deref(),
            Some("services/grpc/suite.lua")
        );
        assert_eq!(r.suites["rest"].setup, None);
    }

    #[test]
    fn parses_registered_sources() {
        let m = Manifest::parse(
            r#"
[run]
paths = ["tests"]

[sources]
acme   = "github:acme"
mirror = "https://git.acme.io/plugins"

[plugins]
redis = "acme:prova-redis@v1"
"#,
        )
        .unwrap();
        let r = m.resolve(None).unwrap();
        assert_eq!(r.sources["acme"], "github:acme");
        assert_eq!(r.sources["mirror"], "https://git.acme.io/plugins");
        assert_eq!(
            r.plugins["redis"],
            PluginSource::Path("acme:prova-redis@v1".into())
        );
    }

    #[test]
    fn profile_overlays_base_and_merges_env() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let r = m.resolve(Some("ci")).unwrap();
        // jobs overridden; paths + format inherited from [run]; env is base-then-profile.
        assert_eq!(r.jobs, Some(8));
        assert_eq!(r.paths, vec!["tests".to_string()]);
        assert_eq!(r.format.as_deref(), Some("console"));
        assert_eq!(r.env, env(&[("CI", "true"), ("LOG", "info")]));
    }

    #[test]
    fn profile_can_override_paths() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let r = m.resolve(Some("smoke")).unwrap();
        assert_eq!(r.paths, vec!["tests/smoke".to_string()]);
        assert_eq!(r.jobs, Some(4)); // inherited
    }

    #[test]
    fn profile_plugins_overlay_project_wide_plugins() {
        let m = Manifest::parse(
            r#"
[run]
paths = ["tests"]

[plugins]
redis = "./plugins/redis.lua"

[profiles.ci]
[profiles.ci.plugins]
kafka = { git = "https://example.com/acme/prova-kafka", tag = "v1" }
redis = "./plugins/redis-ci.lua"
"#,
        )
        .unwrap();

        // Base run: only the project-wide plugin.
        let base = m.resolve(None).unwrap();
        assert_eq!(base.plugins.len(), 1);
        assert_eq!(
            base.plugins["redis"],
            PluginSource::Path("./plugins/redis.lua".into())
        );

        // CI profile: adds kafka, and its redis entry wins over the project-wide one.
        let ci = m.resolve(Some("ci")).unwrap();
        assert_eq!(ci.plugins.len(), 2);
        assert_eq!(
            ci.plugins["kafka"],
            PluginSource::Detailed(PluginDetail {
                git: Some("https://example.com/acme/prova-kafka".into()),
                tag: Some("v1".into()),
                ..Default::default()
            })
        );
        assert_eq!(
            ci.plugins["redis"],
            PluginSource::Path("./plugins/redis-ci.lua".into())
        );
    }

    #[test]
    fn unknown_profile_is_an_error() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert!(m.resolve(Some("nope")).is_err());
    }

    #[test]
    fn empty_manifest_resolves_to_empty_defaults() {
        let m = Manifest::parse("").unwrap();
        let r = m.resolve(None).unwrap();
        assert!(r.paths.is_empty());
        assert_eq!(r.jobs, None);
    }
}
