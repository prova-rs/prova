//! `[capabilities]` — the manifest's capability vocabulary (docs/design/capabilities.md).
//!
//! ```toml
//! [capabilities]
//! "*"     = "error"                                      # fall-through: probe | warn | error
//! docker  = { intrinsic = "docker" }                      # prova's own checker, said out loud
//! gpu     = { package = "env", capability = "gpu" }       # a Lua predicate, from a package
//! java    = { command = "java", version = ["-version"], stream = "stderr" }
//! kubectl = { command = "kubectl", version = ["version", "--client"],
//!             pattern = "GitVersion:\"v([0-9.]+)\"" }
//! ```
//!
//! The same registration grammar `[topologies]` uses, and deliberately so: one shape for "a thing my
//! package provides under a name". This module owns parsing and validation; `prova_core` owns
//! resolution and probing.
//!
//! It lives in its own file rather than in `manifest.rs` because that file sits at the 1500-line
//! quality ratchet — and because the selector validation below is a self-contained concern with real
//! rules, not another `#[serde(default)]` field.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One `[capabilities]` entry: either the `"*"` fall-through policy (a bare string) or a
/// declaration table.
///
/// `untagged` is safe here only because the two arms cannot be confused — a string is never a table —
/// and because the declaration arm denies unknown fields, so a typo'd key cannot fall out of the
/// table arm and get silently reinterpreted.
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum CapabilityEntry {
    /// `"*" = "error"` — what an undeclared name means here.
    Policy(String),
    /// A named capability's factory.
    Decl(Box<CapabilityDecl>),
}

/// A `[capabilities]` declaration. Exactly one selector — `package`, `command`, or `intrinsic` —
/// must be present; the rest of the keys configure whichever one it is.
#[derive(Debug, Deserialize, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDecl {
    // --- the `package` selector ---
    /// The package providing the predicate — a name in `[dependencies]` or an ambient package.
    pub package: Option<String>,
    /// The package's advertised capability name (`[[package.capabilities]]`). Mutually exclusive
    /// with `factory`.
    pub capability: Option<String>,
    /// A direct dotted path to the predicate inside the package's namespace. Mutually exclusive
    /// with `capability`.
    pub factory: Option<String>,
    /// Passed to the predicate as its argument, so one generic factory can serve several
    /// capabilities. Package selector only — a command probe is already fully declared by its keys.
    #[serde(default)]
    pub options: toml::Table,

    // --- the `command` selector ---
    /// The executable. Also the PATH-presence check when `probe` is absent.
    pub command: Option<String>,
    /// Args for the availability check; exit 0 means available.
    pub probe: Option<Vec<String>>,
    /// Require `probe`'s output (on `stream`) to equal this, trimmed, case-insensitively.
    pub expect: Option<String>,
    /// `false` for "no version concept", or the args that produce one. Absent → the `--version`
    /// heuristic.
    pub version: Option<VersionSpec>,
    /// Which stream carries the output: `"stdout"` (default), `"stderr"`, `"both"`.
    pub stream: Option<String>,
    /// A regex over that output; first capture group, else the whole match.
    pub pattern: Option<String>,
    /// Retry the availability probe this many times, with backoff.
    pub retries: Option<u32>,

    // --- the `intrinsic` selector ---
    /// One of prova's built-in checkers, by name.
    pub intrinsic: Option<String>,
}

/// `version = false` or `version = [...]`.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum VersionSpec {
    Disabled(bool),
    Args(Vec<String>),
}

impl CapabilityDecl {
    /// Which selector this entry names, or a diagnostic saying it named none or several.
    ///
    /// Refusing "several" rather than picking a precedence is the same rule `[topologies]` applies to
    /// `topology` vs `factory`: an entry naming two is an author who is unsure, and resolving it
    /// quietly means the one they did not mean silently wins.
    fn selector(&self, name: &str) -> Result<Selector, String> {
        let named: Vec<&str> = [
            self.package.as_ref().map(|_| "package"),
            self.command.as_ref().map(|_| "command"),
            self.intrinsic.as_ref().map(|_| "intrinsic"),
        ]
        .into_iter()
        .flatten()
        .collect();
        match named.as_slice() {
            ["package"] => Ok(Selector::Package),
            ["command"] => Ok(Selector::Command),
            ["intrinsic"] => Ok(Selector::Intrinsic),
            [] => Err(format!(
                "[capabilities] {name}: needs a selector — one of `package = \"<pkg>\"` (a Lua \
                 predicate), `command = \"<exe>\"` (a declarative probe), or `intrinsic = \
                 \"<builtin>\"` (one of prova's own checkers). See `prova learn capabilities`."
            )),
            several => Err(format!(
                "[capabilities] {name}: names {} selectors ({}) — exactly one, so it is clear which \
                 factory answers for this capability",
                several.len(),
                several.join(", ")
            )),
        }
    }

    /// Reject keys that belong to a different selector. Ignoring them would be the manifest's
    /// version of a dropped option: `{ command = "java", capability = "gpu" }` looks like it wired a
    /// predicate and did not.
    fn reject_foreign_keys(&self, name: &str, selector: Selector) -> Result<(), String> {
        let package_keys = [
            self.capability.as_ref().map(|_| "capability"),
            self.factory.as_ref().map(|_| "factory"),
            (!self.options.is_empty()).then_some("options"),
        ];
        let command_keys = [
            self.probe.as_ref().map(|_| "probe"),
            self.expect.as_ref().map(|_| "expect"),
            self.version.as_ref().map(|_| "version"),
            self.stream.as_ref().map(|_| "stream"),
            self.pattern.as_ref().map(|_| "pattern"),
            self.retries.as_ref().map(|_| "retries"),
        ];
        let (foreign, owner): (Vec<&str>, &str) = match selector {
            Selector::Package => (command_keys.into_iter().flatten().collect(), "command"),
            Selector::Command => (package_keys.into_iter().flatten().collect(), "package"),
            Selector::Intrinsic => (
                package_keys
                    .into_iter()
                    .chain(command_keys)
                    .flatten()
                    .collect(),
                "package/command",
            ),
        };
        if foreign.is_empty() {
            return Ok(());
        }
        Err(format!(
            "[capabilities] {name}: `{}` {} a `{owner}` key, but this entry uses the `{}` selector",
            foreign.join("`, `"),
            if foreign.len() == 1 { "is" } else { "are" },
            selector.label(),
        ))
    }

    /// Resolve to the engine's registration, validating as we go.
    fn resolve(&self, name: &str) -> Result<prova_core::CapabilityRegistration, String> {
        let selector = self.selector(name)?;
        self.reject_foreign_keys(name, selector)?;
        let factory = match selector {
            Selector::Intrinsic => {
                // Unwrap-free: `selector()` returned Intrinsic only because this is Some.
                let preset = self.intrinsic.clone().unwrap_or_default();
                prova_core::CapabilityFactory::Intrinsic(preset)
            }
            Selector::Package => {
                let package = self.package.clone().unwrap_or_default();
                // `capability = "gpu"` is the encapsulated form (the package advertises the name and
                // owns the path); `factory = "capabilities.gpu"` reaches straight in. Same two doors
                // `[topologies]` offers, same reason: a published package mediates, your own need not.
                let factory = match (&self.capability, &self.factory) {
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "[capabilities] {name}: `capability` and `factory` are mutually \
                             exclusive — name the package's advertised capability, or a dotted path \
                             into it, not both"
                        ))
                    }
                    (Some(advertised), None) => format!("capabilities.{advertised}"),
                    (None, Some(path)) => path.clone(),
                    (None, None) => {
                        return Err(format!(
                            "[capabilities] {name}: `package = {package:?}` needs `capability = \
                             \"<advertised-name>\"` (the package's own name for it) or `factory = \
                             \"<dotted.path>\"` (a path into its namespace)"
                        ))
                    }
                };
                prova_core::CapabilityFactory::Package {
                    package,
                    factory,
                    options: crate::cmd_topo::topology_options_to_lua(&self.options),
                }
            }
            Selector::Command => {
                let version = match &self.version {
                    None => prova_core::VersionQuery::Heuristic,
                    Some(VersionSpec::Disabled(false)) => prova_core::VersionQuery::None,
                    Some(VersionSpec::Disabled(true)) => {
                        return Err(format!(
                            "[capabilities] {name}: `version = true` says nothing — omit the key for \
                             the `--version` default, give the args that produce a version, or say \
                             `version = false` for a capability that has none"
                        ))
                    }
                    Some(VersionSpec::Args(args)) => {
                        if args.is_empty() {
                            return Err(format!(
                                "[capabilities] {name}: `version = []` has no args — omit the key \
                                 for the `--version` default, or say `version = false` for a \
                                 capability that has no version"
                            ));
                        }
                        prova_core::VersionQuery::Args(args.clone())
                    }
                };
                let stream = match &self.stream {
                    None => prova_core::Stream::default(),
                    Some(s) => prova_core::Stream::parse(s)
                        .map_err(|e| format!("[capabilities] {name}: {e}"))?,
                };
                prova_core::CapabilityFactory::Command(prova_core::CommandProbe {
                    command: self.command.clone().unwrap_or_default(),
                    probe: self.probe.clone(),
                    expect: self.expect.clone(),
                    version,
                    stream,
                    pattern: self.pattern.clone(),
                    retries: self.retries.unwrap_or(1),
                })
            }
        };
        Ok(prova_core::CapabilityRegistration {
            name: name.to_string(),
            factory,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Selector {
    Package,
    Command,
    Intrinsic,
}

impl Selector {
    fn label(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Command => "command",
            Self::Intrinsic => "intrinsic",
        }
    }
}

/// The `[capabilities]` table, resolved: the registrations and the fall-through policy.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResolvedCapabilities {
    pub registrations: Vec<prova_core::CapabilityRegistration>,
    pub undeclared: prova_core::UndeclaredPolicy,
}

/// Resolve `[capabilities]`, refusing anything ambiguous.
///
/// A whole-section error rather than a per-entry skip: a capability that silently failed to declare
/// would leave every test gated on it skipping, and the run green.
pub fn resolve(
    table: &BTreeMap<String, CapabilityEntry>,
) -> Result<ResolvedCapabilities, String> {
    let mut out = ResolvedCapabilities::default();
    for (name, entry) in table {
        match entry {
            CapabilityEntry::Policy(value) => {
                // A bare string is only ever the wildcard. A capability named with a string value
                // (`docker = "intrinsic"`) is a shape confusion worth naming, not a policy.
                if name != "*" {
                    return Err(format!(
                        "[capabilities] {name} = {value:?}: a capability is declared with a table, \
                         not a string — e.g. {name} = {{ command = {name:?} }}. (Only \"*\", the \
                         fall-through policy, takes a bare string.)"
                    ));
                }
                out.undeclared = prova_core::UndeclaredPolicy::parse(value)?;
            }
            CapabilityEntry::Decl(decl) => {
                if name == "*" {
                    return Err(
                        "[capabilities] \"*\" is the fall-through policy, not a capability — its \
                         value is a string: \"probe\", \"warn\", or \"error\""
                            .to_string(),
                    );
                }
                out.registrations.push(decl.resolve(name)?);
            }
        }
    }
    Ok(out)
}

/// A package's declared vocabulary: the `[capabilities]` factories (plus whatever the deprecated
/// companion still registers), resolved and probeable, with a row per name saying where it came from.
///
/// Shared by `prova capabilities` and the MCP `capabilities` tool so the two cannot disagree about
/// what a name MEANS here — which they would the moment one of them stayed host-only while the
/// other honored an override.
///
/// Resolved leniently, unlike a run: this is a report, and a report that refuses to print because
/// one predicate's package is unreachable is worse than one that says so per row.
pub fn project_vocabulary(
    home: &crate::home::Home,
    m: &crate::manifest::Manifest,
) -> (prova_core::Capabilities, Vec<(String, String)>) {
    let mut caps = prova_core::Capabilities::default();
    let mut origins: Vec<(String, String)> = Vec::new();

    // The project's own AMBIENT packages (the `packages` dir — a directory scan, no network) so a
    // `package`-kind capability declared against them probes for real. Git-sourced dependencies are
    // deliberately not fetched: a report must not reach the network.
    let pkgs = crate::packages::ResolvedPackages {
        search_root: m.run.packages.as_ref().map(|d| home.dir.join(d)),
        ..Default::default()
    };
    let engine = crate::suites::engine_config(1, &pkgs, Some(home), prova_core::progress::null());

    // The deprecated companion first, so the manifest layers on top of it — the same precedence a
    // run applies (docs/design/capabilities.md#manifest-wins-over-the-companion).
    let companion_rel = m
        .run
        .config
        .clone()
        .unwrap_or_else(|| "prova.lua".to_string());
    let companion = home.dir.join(&companion_rel);
    if companion.is_file() {
        if let Ok(loaded) = prova_core::load_project_config(&companion, &engine) {
            for name in loaded.registered_names() {
                origins.push((
                    name.clone(),
                    "registered in the companion (deprecated)".to_string(),
                ));
            }
            caps = loaded;
        }
    }

    let Ok(declared) = resolve(&m.capabilities) else {
        return (caps, origins);
    };
    let (resolved, unresolved) = match prova_core::resolve_capabilities(
        &declared.registrations,
        declared.undeclared,
        &engine,
    ) {
        Ok(all) => (all, Vec::new()),
        // One unreachable package predicate must not take the whole section down with it — the
        // command and intrinsic declarations resolve from data alone and are still true. Report
        // those, and name the ones that could not be reached rather than omitting them: a capability
        // missing from the report reads as "not declared", which is a different fact.
        Err(_) => {
            let (data_only, pkg_kind): (Vec<_>, Vec<_>) =
                declared.registrations.iter().cloned().partition(|r| {
                    !matches!(r.factory, prova_core::CapabilityFactory::Package { .. })
                });
            let partial =
                prova_core::resolve_capabilities(&data_only, declared.undeclared, &engine)
                    .unwrap_or_default();
            (partial, pkg_kind)
        }
    };
    for (name, kind) in resolved.declared_names() {
        // An overridden built-in is never described as an ordinary declaration: the name no longer
        // means what a reader of another repo would assume, and saying so is what makes overriding
        // safe (docs/design/capabilities.md#overriding-a-builtin-is-declared).
        let origin = if resolved.overrides_builtin(&name) {
            format!("{} — OVERRIDES the built-in", kind.label())
        } else {
            format!("declared: {}", kind.label())
        };
        origins.push((name, origin));
    }
    for reg in &unresolved {
        origins.push((
            reg.name.clone(),
            "declared: package predicate (UNRESOLVED here)".to_string(),
        ));
    }
    caps.absorb(resolved);
    (caps, origins)
}

/// Teach the declarations a `"*" = "warn"` run fell through to.
///
/// Printed once, after the run, as one grouped block — rather than at each reference, interleaved
/// with test output. `warn` is the migration rung: its whole job is to hand back a list an author can
/// paste, so it must read as a list.
pub fn teach_undeclared(names: &[String]) {
    if names.is_empty() {
        return;
    }
    eprintln!();
    eprintln!(
        "prova: {} undeclared {} probed on PATH ([capabilities] \"*\" = \"warn\"). Declare them:",
        names.len(),
        if names.len() == 1 {
            "capability was"
        } else {
            "capabilities were"
        }
    );
    eprintln!();
    eprintln!("    [capabilities]");
    for name in names {
        eprintln!("    {name} = {{ command = {name:?} }}");
    }
    eprintln!();
    eprintln!(
        "prova: then set \"*\" = \"error\" to close the vocabulary. (`prova learn capabilities`)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> Result<ResolvedCapabilities, String> {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            capabilities: BTreeMap<String, CapabilityEntry>,
        }
        let w: Wrapper = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        resolve(&w.capabilities)
    }

    /// The three selectors each resolve to their own factory kind — the whole point of the section.
    #[test]
    fn each_selector_resolves_to_its_factory() {
        let r = parse(
            "[capabilities]\n\
             docker = { intrinsic = \"docker\" }\n\
             gpu = { package = \"env\", capability = \"gpu\" }\n\
             java = { command = \"java\", version = [\"-version\"], stream = \"stderr\" }\n",
        )
        .expect("resolves");
        assert_eq!(r.registrations.len(), 3);
        let by_name = |n: &str| {
            r.registrations
                .iter()
                .find(|reg| reg.name == n)
                .map(|reg| reg.factory.clone())
                .expect("registered")
        };
        assert_eq!(
            by_name("docker"),
            prova_core::CapabilityFactory::Intrinsic("docker".into())
        );
        // The advertised name resolves through the package's `capabilities` namespace, so a package
        // author owns the path and the consumer names only what was advertised.
        assert_eq!(
            by_name("gpu"),
            prova_core::CapabilityFactory::Package {
                package: "env".into(),
                factory: "capabilities.gpu".into(),
                options: None,
            }
        );
        match by_name("java") {
            prova_core::CapabilityFactory::Command(p) => {
                assert_eq!(p.command, "java");
                assert_eq!(p.stream, prova_core::Stream::Stderr);
                assert_eq!(
                    p.version,
                    prova_core::VersionQuery::Args(vec!["-version".into()])
                );
            }
            other => panic!("expected a command probe, got {other:?}"),
        }
    }

    /// `"*"` is the policy, and the default is the permissive behavior every existing manifest has.
    #[test]
    fn wildcard_sets_the_fall_through_policy() {
        let r = parse("[capabilities]\n\"*\" = \"error\"\n").expect("resolves");
        assert_eq!(r.undeclared, prova_core::UndeclaredPolicy::Error);
        assert!(r.registrations.is_empty(), "the policy is not a capability");
        let none = parse("[capabilities]\nkind = { command = \"kind\" }\n").expect("resolves");
        assert_eq!(none.undeclared, prova_core::UndeclaredPolicy::Probe);
    }

    /// An entry with no selector, or two, is refused — never resolved by precedence.
    #[test]
    fn a_selectorless_or_ambiguous_entry_is_refused() {
        let none = parse("[capabilities]\nx = { retries = 2 }\n").expect_err("refused");
        assert!(none.contains("needs a selector"), "{none}");
        let two = parse("[capabilities]\nx = { command = \"c\", intrinsic = \"docker\" }\n")
            .expect_err("refused");
        assert!(two.contains("2 selectors"), "{two}");
    }

    /// A key belonging to another selector is refused rather than ignored: it reads as wired and
    /// would not be.
    #[test]
    fn foreign_keys_are_refused() {
        let e = parse("[capabilities]\nx = { command = \"c\", capability = \"gpu\" }\n")
            .expect_err("refused");
        assert!(e.contains("capability"), "{e}");
        assert!(e.contains("command"), "{e}");
    }

    /// `version = true` says nothing; `version = false` says something specific.
    #[test]
    fn version_true_is_refused_and_false_means_no_version() {
        let e = parse("[capabilities]\nx = { command = \"c\", version = true }\n")
            .expect_err("refused");
        assert!(e.contains("says nothing"), "{e}");
        let r = parse("[capabilities]\nx = { command = \"c\", version = false }\n")
            .expect("resolves");
        match &r.registrations[0].factory {
            prova_core::CapabilityFactory::Command(p) => {
                assert_eq!(p.version, prova_core::VersionQuery::None);
            }
            other => panic!("expected a command probe, got {other:?}"),
        }
    }

    /// An unknown key is a config error with a name in it, not a silently dropped setting.
    #[test]
    fn an_unknown_key_is_refused() {
        let e = parse("[capabilities]\nx = { command = \"c\", vershion = [\"-v\"] }\n")
            .expect_err("refused");
        assert!(e.contains("vershion"), "{e}");
    }

    /// A capability declared with a bare string is the shape confusion the untagged enum could
    /// otherwise swallow as a policy.
    #[test]
    fn a_string_valued_capability_is_refused() {
        let e = parse("[capabilities]\ndocker = \"intrinsic\"\n").expect_err("refused");
        assert!(e.contains("declared with a table"), "{e}");
    }

    /// `capability` and `factory` are the two doors into a package, and naming both is unsure.
    #[test]
    fn capability_and_factory_are_mutually_exclusive() {
        let e = parse(
            "[capabilities]\nx = { package = \"p\", capability = \"c\", factory = \"a.b\" }\n",
        )
        .expect_err("refused");
        assert!(e.contains("mutually exclusive"), "{e}");
    }
}
