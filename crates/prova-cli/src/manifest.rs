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
use std::time::Duration;

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
    /// Named topologies (`[topologies]`) — each exposes a factory a plugin provides under a project
    /// name, so `prova up <name>` (and any proof) can address it. Sugar for
    /// `prova.topology(<name>, require(<plugin>).<factory>)`. A property of the project, not a profile.
    #[serde(default)]
    pub topologies: BTreeMap<String, TopologyDecl>,
    /// How prova manages the project's LuaLS IDE integration (`.luarc.json` + synced annotations).
    /// Not profile-specific — a property of the project.
    #[serde(default)]
    pub luals: Luals,
    /// How often git plugin sources are checked for updates, and whether to force them. Not
    /// profile-specific — a property of the project. Mirrors archetect's `updates` config so the two
    /// tools read the same knobs.
    #[serde(default)]
    pub updates: UpdatesSection,
}

/// Git-source update policy (`[updates]`). Governs the shared cache's freshness gate for `[plugins]`
/// git sources: within `interval` the cache is used with no network; past it, a cheap `ls-remote`
/// decides whether to actually pull. `force` (also `-U`/`--update`) skips the gate entirely.
#[derive(Debug, Deserialize, Clone, Default, PartialEq)]
pub struct UpdatesSection {
    /// A human duration — `"7d"`, `"12h"`, `"30m"`, `"3600s"`, or a bare integer (seconds). Defaults
    /// to 7 days when absent.
    pub interval: Option<String>,
    /// Force updates, ignoring the freshness gates. The CLI `-U`/`--update` flag also sets this.
    pub force: Option<bool>,
}

impl UpdatesSection {
    /// The default freshness interval when `[updates] interval` is absent: 7 days (matches archetect).
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(604_800);

    /// Resolve `interval` to a `Duration`, defaulting to [`Self::DEFAULT_INTERVAL`]. An unparseable
    /// value is an error the caller surfaces.
    pub fn interval_duration(&self) -> Result<Duration, String> {
        match &self.interval {
            None => Ok(Self::DEFAULT_INTERVAL),
            Some(s) => parse_duration(s),
        }
    }

    /// Whether `[updates] force` is set (default false).
    pub fn force(&self) -> bool {
        self.force.unwrap_or(false)
    }
}

/// Parse a human duration: a bare integer is seconds; a trailing `d`/`h`/`m`/`s` scales it. Keeps
/// the manifest friendly (`"7d"`) without pulling in a date-parsing dependency.
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, scale) = match s.strip_suffix(|c: char| c.is_ascii_alphabetic()) {
        Some(prefix) => (prefix, s.as_bytes()[s.len() - 1].to_ascii_lowercase()),
        None => (s, b's'),
    };
    let value: u64 = num.trim().parse().map_err(|_| {
        format!("invalid updates.interval {s:?} (expected e.g. \"7d\", \"12h\", \"3600s\")")
    })?;
    let secs = match scale {
        b's' => value,
        b'm' => value * 60,
        b'h' => value * 3600,
        b'd' => value * 86_400,
        other => {
            return Err(format!(
                "invalid updates.interval unit {:?} in {s:?} (use d/h/m/s)",
                other as char
            ))
        }
    };
    Ok(Duration::from_secs(secs))
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

/// A named topology (`[topologies] <name> = { plugin = "...", … }`). It names one of a plugin's
/// topologies in one of two ways — exactly one must be given:
///
/// - `topology = "linux-vm"` — by the plugin's advertised NAME (`[[plugin.topologies]]`). The public,
///   encapsulated form: the plugin author owns the factory path; you pick from what's advertised.
/// - `factory = "topologies.linux_vm"` — by a direct dotted path into the plugin's namespace. Handy
///   for your own plugins, where there's no contract to mediate.
///
/// Either way the entry desugars to `prova.topology("<name>", require("<plugin>").<factory>)`.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TopologyDecl {
    /// The plugin that provides it — a name declared in `[plugins]` or an ambient plugin under the
    /// `plugin_root`.
    pub plugin: String,
    /// The plugin's advertised topology name (`[[plugin.topologies]]`). Mutually exclusive with
    /// `factory`.
    pub topology: Option<String>,
    /// A direct dotted path to the factory inside the plugin's namespace. Mutually exclusive with
    /// `topology`.
    pub factory: Option<String>,
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
    /// The directory holding this project's own plugins, relative to the project **root** (like
    /// `paths`). No default: the one place prova scans is the one this file names.
    ///
    /// Deliberately singular. An ambient root exists for a single job — "my project's plugins, don't
    /// make me name each one" — and that is inherently one place. A plugin from anywhere else gets a
    /// name and a pinned source in `[plugins]` (a path or a git ref), which is both more explicit and
    /// more reproducible than a second directory scanned by convention. A list would only add a
    /// precedence question ("two roots both hold `foo` — which wins?") without adding capability.
    pub plugin_root: Option<String>,
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
    /// The project's plugin directory (`[run] plugin_root`), relative to the project root. `None`
    /// means the project declared none — the searcher then has nowhere to scan, and says so.
    pub plugin_root: Option<String>,
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
    /// Named topologies (`[topologies]`) — name → the plugin factory it exposes.
    pub topologies: BTreeMap<String, TopologyDecl>,
    /// LuaLS IDE-integration policy (`[luals]`).
    pub luals: Luals,
    /// Git-source update policy (`[updates]`), applied to every run.
    pub updates: UpdatesSection,
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
        // A profile that names a root means "scan here instead" — it replaces, never adds, so
        // selecting a profile can not widen resolution.
        let plugin_root = overlay
            .and_then(|p| p.plugin_root.clone())
            .or_else(|| base.plugin_root.clone());
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
            plugin_root,
            config,
            jobs,
            format,
            env,
            suites: self.suites.clone(),
            plugins,
            sources: self.sources.clone(),
            topologies: self.topologies.clone(),
            luals: self.luals.clone(),
            updates: self.updates.clone(),
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
                plugin_root: None,
                config: None,
                jobs: Some(4),
                format: Some("console".into()),
                env: env(&[("LOG", "info")]),
                suites: BTreeMap::new(),
                plugins: BTreeMap::new(),
                sources: BTreeMap::new(),
                topologies: BTreeMap::new(),
                luals: Luals::default(),
                updates: UpdatesSection::default(),
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

    /// `[topologies]` maps a project name to a plugin factory — the desugaring surface for
    /// `prova up <name>`. A property of the project (not profile-specific), and the entry form is
    /// strict (`deny_unknown_fields`) so a typo'd key is a parse error, not a silently-ignored one.
    #[test]
    fn topologies_parse_as_plugin_factory_references() {
        let m = Manifest::parse(
            "[run]\npaths = [\"proofs\"]\n\n\
             [topologies]\n\
             vm = { plugin = \"parallels\", factory = \"topologies.linux_vm\" }\n",
        )
        .unwrap();
        let t = &m.resolve(None).unwrap().topologies;
        assert_eq!(t.len(), 1);
        assert_eq!(t["vm"].plugin, "parallels");
        assert_eq!(t["vm"].factory.as_deref(), Some("topologies.linux_vm"));
        assert_eq!(t["vm"].topology, None);
    }

    /// `plugin_root` is the whole of ambient on-disk plugin resolution, so each half matters: absent
    /// means *nothing* is scanned (never a silent default — that is the point of declaring it), and a
    /// profile that names one replaces rather than adds, so selecting a profile cannot widen
    /// resolution. It is singular by design: a second ambient directory would only add a precedence
    /// question, while a plugin from elsewhere belongs in `[plugins]` with a name and a pinned source.
    #[test]
    fn plugin_root_is_declared_and_profile_overridable() {
        let m = Manifest::parse(
            r#"
[run]
paths = ["proofs"]
plugin_root = ".prova/plugins"

[profiles.vendored]
plugin_root = "vendor/plugins"

[profiles.smoke]
paths = ["proofs/smoke"]
"#,
        )
        .unwrap();

        assert_eq!(
            m.resolve(None).unwrap().plugin_root.as_deref(),
            Some(".prova/plugins")
        );
        // Replaced, not added to.
        assert_eq!(
            m.resolve(Some("vendored")).unwrap().plugin_root.as_deref(),
            Some("vendor/plugins")
        );
        // A profile silent about the root inherits the project's.
        assert_eq!(
            m.resolve(Some("smoke")).unwrap().plugin_root.as_deref(),
            Some(".prova/plugins")
        );
        // No `plugin_root` anywhere means nothing is scanned — no built-in fallback.
        let bare = Manifest::parse("[run]\npaths = [\"proofs\"]\n").unwrap();
        assert!(bare.resolve(None).unwrap().plugin_root.is_none());
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
