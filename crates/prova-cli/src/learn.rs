//! `prova learn` — the progressive-disclosure topic catalog (docs/plans/autodidact.md M1).
//!
//! The embedded skill is the entry point; depth lives here, one screen per topic, so an agent
//! learns Prova from the binary alone — no source tree, no docs site. Topics are static doctrine
//! (embedded markdown) plus **dynamic slots** (`{{slot}}`) computed from the resolved package at
//! the moment of asking, so a topic is always true for THIS project and degrades imperatively
//! when there is no manifest in reach.
//!
//! Invalid states are unrepresentable where the type system can manage it: a [`Topic`] without
//! content cannot compile (`include_str!` per variant, exhaustive matches), the slot vocabulary
//! is a closed enum, and the in-crate tests close the rest (every `{{slot}}` parses, every topic
//! titles itself, aliases never collide). See docs/plans/autodidact.md §2.8.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use crate::catalog::Catalog;
use crate::home;
use crate::manifest::{Manifest, PluginSource, Profile, Resolved};

/// Every topic the catalog serves. Adding a variant without a markdown file (or vice versa) is a
/// compile error; forgetting it in a match is too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Pdd,
    Project,
    Init,
    Doubles,
}

impl Topic {
    pub const ALL: &'static [Topic] = &[Topic::Pdd, Topic::Project, Topic::Init, Topic::Doubles];

    /// Intuitive names resolve instead of bouncing off our taxonomy (`prova learn mocks` works).
    /// Collisions with keys or each other are forbidden by test.
    const ALIASES: &'static [(&'static str, Topic)] = &[
        ("mocks", Topic::Doubles),
        ("mock", Topic::Doubles),
        ("containers", Topic::Doubles),
        ("manifest", Topic::Project),
        ("layout", Topic::Project),
        ("package", Topic::Project),
        ("scaffold", Topic::Init),
        ("archetype", Topic::Init),
        ("archetypes", Topic::Init),
        ("tdd", Topic::Pdd),
        ("proof-driven-development", Topic::Pdd),
    ];

    pub fn key(self) -> &'static str {
        match self {
            Topic::Pdd => "pdd",
            Topic::Project => "project",
            Topic::Init => "init",
            Topic::Doubles => "doubles",
        }
    }

    /// The embedded doctrine. One file per variant; the pairing is what makes an undocumented
    /// topic unrepresentable.
    fn source(self) -> &'static str {
        match self {
            Topic::Pdd => include_str!("topics/pdd.md"),
            Topic::Project => include_str!("topics/project.md"),
            Topic::Init => include_str!("topics/init.md"),
            Topic::Doubles => include_str!("topics/doubles.md"),
        }
    }

    /// The one-line hook shown in the listing — parsed from the topic's own title line
    /// (`# <key> — <hook>`), so it is written exactly once. Format enforced by test.
    pub fn hook(self) -> &'static str {
        let first = self.source().lines().next().unwrap_or("");
        match first.split_once(" — ") {
            Some((_, hook)) => hook,
            None => first,
        }
    }

    /// The raw embedded source, for the crate's reference lint (`prova <verb>` mentions must be
    /// real verbs). Test-only by convention; the renderer is the real read path.
    #[cfg(test)]
    pub fn rendered_source_for_lint(self) -> &'static str {
        self.source()
    }

    pub fn resolve(input: &str) -> Option<Topic> {
        let needle = input.trim().to_lowercase();
        Topic::ALL
            .iter()
            .copied()
            .find(|t| t.key() == needle)
            .or_else(|| {
                Topic::ALIASES
                    .iter()
                    .find(|(alias, _)| *alias == needle)
                    .map(|(_, t)| *t)
            })
    }
}

/// Which surface is asking. The truth is identical; the SPELLING of moves is not — an MCP agent
/// calls tools, a CLI agent runs commands, and each learns the other exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Cli,
    Mcp,
}

/// The closed slot vocabulary. A `{{name}}` outside this enum fails the in-crate tests, and every
/// variant must render (exhaustive match), including its no-package degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    InitCatalog,
    ProofPaths,
    PluginRoot,
    Plugins,
    Topologies,
    Profiles,
}

impl Slot {
    fn parse(name: &str) -> Option<Slot> {
        match name {
            "init_catalog" => Some(Slot::InitCatalog),
            "proof_paths" => Some(Slot::ProofPaths),
            "plugin_root" => Some(Slot::PluginRoot),
            "plugins" => Some(Slot::Plugins),
            "topologies" => Some(Slot::Topologies),
            "profiles" => Some(Slot::Profiles),
            _ => None,
        }
    }
}

/// The package the renderer computes dynamic facts from — resolved fresh per render, so the
/// answer is true at the moment of asking.
struct PackageFacts {
    manifest_name: String,
    resolved: Resolved,
    profiles: BTreeMap<String, Profile>,
}

/// What the renderer knows about where it is running.
pub struct RenderEnv {
    package: Option<PackageFacts>,
    /// A manifest that exists but cannot be loaded is surfaced, never silently treated as absent.
    problem: Option<String>,
}

impl RenderEnv {
    /// Resolve from a starting directory by walking up, exactly like a run would.
    pub fn at(start: &Path) -> RenderEnv {
        let found = match home::find(start) {
            Ok(h) => h,
            Err(e) => return RenderEnv { package: None, problem: Some(e) },
        };
        let Some(home) = found else {
            return RenderEnv { package: None, problem: None };
        };
        let load = std::fs::read_to_string(&home.manifest)
            .map_err(|e| format!("cannot read {}: {e}", home.manifest.display()))
            .and_then(|text| Manifest::parse(&text))
            .and_then(|m| {
                let resolved = m.resolve(None)?;
                Ok(PackageFacts {
                    manifest_name: home
                        .manifest
                        .strip_prefix(&home.dir)
                        .unwrap_or(&home.manifest)
                        .display()
                        .to_string(),
                    resolved,
                    profiles: m.profiles,
                })
            });
        match load {
            Ok(p) => RenderEnv { package: Some(p), problem: None },
            Err(e) => RenderEnv { package: None, problem: Some(e) },
        }
    }

    fn no_package_line(&self, transport: Transport) -> String {
        if let Some(problem) = &self.problem {
            return format!("A manifest was found but could not be loaded: {problem}");
        }
        let init = match transport {
            Transport::Cli => "run `prova init`",
            Transport::Mcp => "run `prova init` via the shell (no MCP tool scaffolds)",
        };
        format!(
            "No prova.toml found from the working directory — {init} to scaffold a package \
             (see `prova learn init`), or work from inside one."
        )
    }
}

/// One plugin source, described the way an agent would re-declare it.
fn describe_source(source: &PluginSource) -> String {
    match source {
        PluginSource::Path(s) => s.clone(),
        PluginSource::Detailed(d) => {
            let origin = d
                .git
                .clone()
                .or_else(|| d.path.clone())
                .unwrap_or_default();
            let pin = [("tag", &d.tag), ("branch", &d.branch), ("rev", &d.rev)]
                .into_iter()
                .find_map(|(k, v)| v.as_ref().map(|v| format!(" ({k} {v})")))
                .unwrap_or_default();
            format!("{origin}{pin}")
        }
    }
}

fn render_slot(slot: Slot, env: &RenderEnv, transport: Transport) -> String {
    match slot {
        Slot::InitCatalog => {
            let layout = prova_core::XdgSystemLayout::new()
                .map_err(|e| e.to_string())
                .and_then(|l| Catalog::load(&l));
            match layout {
                Ok(catalog) => {
                    let width = catalog.entries.keys().map(String::len).max().unwrap_or(0);
                    let mut out: Vec<String> = catalog
                        .entries
                        .iter()
                        .map(|(key, entry)| format!("  {key:<width$}  {}", entry.description))
                        .collect();
                    out.push(String::new());
                    out.push(match transport {
                        Transport::Cli => {
                            "Render one: `prova init <key>` (`--headless` in automation).".into()
                        }
                        Transport::Mcp => {
                            "Render one by shelling out: `prova init <key> --headless` — no MCP \
                             tool scaffolds a package."
                                .into()
                        }
                    });
                    out.join("\n")
                }
                Err(e) => format!("The init catalog could not be loaded: {e}"),
            }
        }
        Slot::ProofPaths => match &env.package {
            Some(p) => format!(
                "**Proofs** ({}): `proofs = [{}]` — directory-NAME patterns; every matching \
                 directory below the package root holds `*_test.lua` proofs. Put new proofs there.",
                p.manifest_name,
                p.resolved
                    .proofs
                    .iter()
                    .map(|s| format!("\"{s}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            None => env.no_package_line(transport),
        },
        Slot::PluginRoot => match &env.package {
            Some(p) => match &p.resolved.plugin_root {
                Some(root) => format!(
                    "**Local plugins**: author them under `{root}/<name>/` (the declared \
                     `plugin_root`); each subdirectory is requirable by name."
                ),
                None => "**Local plugins**: no `plugin_root` declared — set `[run] plugin_root` \
                         in the manifest before authoring package-local plugins."
                    .into(),
            },
            None => String::new(),
        },
        Slot::Plugins => match &env.package {
            Some(p) if !p.resolved.plugins.is_empty() => {
                let width = p.resolved.plugins.keys().map(String::len).max().unwrap_or(0);
                let rows: Vec<String> = p
                    .resolved
                    .plugins
                    .iter()
                    .map(|(name, src)| format!("  {name:<width$}  {}", describe_source(src)))
                    .collect();
                format!(
                    "**Declared plugins** (`require(\"<name>\")` in any proof):\n{}",
                    rows.join("\n")
                )
            }
            Some(_) => "**Declared plugins**: none — add them under `[plugins]` in the manifest."
                .into(),
            None => env.no_package_line(transport),
        },
        Slot::Topologies => match &env.package {
            Some(p) if !p.resolved.topologies.is_empty() => {
                let rows: Vec<String> = p
                    .resolved
                    .topologies
                    .iter()
                    .map(|(name, t)| {
                        let what = t
                            .topology
                            .as_ref()
                            .map(|s| format!("topology `{s}`"))
                            .or_else(|| t.factory.as_ref().map(|s| format!("factory `{s}`")))
                            .unwrap_or_default();
                        let requires = if t.requires.is_empty() {
                            String::new()
                        } else {
                            format!("  (requires {})", t.requires.join(", "))
                        };
                        format!("  {name}  → plugin `{}` {what}{requires}", t.plugin)
                    })
                    .collect();
                let verb = match transport {
                    Transport::Cli => "`prova up <name>` holds one live; proofs `t:use` the same definition",
                    Transport::Mcp => "`up { name }` holds one warm in the server; `run`/`eval` with `topology` then hit it",
                };
                format!("**Topologies**:\n{}\n  {verb}.", rows.join("\n"))
            }
            Some(_) => "**Topologies**: none declared (`[topologies]` names a plugin's factory so \
                        `up` and proofs share one environment)."
                .into(),
            None => String::new(),
        },
        Slot::Profiles => match &env.package {
            Some(p) if !p.profiles.is_empty() => {
                let rows: Vec<String> = p
                    .profiles
                    .iter()
                    .map(|(name, profile)| {
                        let mut overrides: Vec<&str> = Vec::new();
                        if !profile.proofs.is_empty() {
                            overrides.push("proofs");
                        }
                        if profile.plugin_root.is_some() {
                            overrides.push("plugin_root");
                        }
                        if profile.config.is_some() {
                            overrides.push("config");
                        }
                        if profile.jobs.is_some() {
                            overrides.push("jobs");
                        }
                        if profile.format.is_some() {
                            overrides.push("format");
                        }
                        if !profile.env.is_empty() {
                            overrides.push("env");
                        }
                        if !profile.plugins.is_empty() {
                            overrides.push("plugins");
                        }
                        if !profile.must_run.is_empty() {
                            overrides.push("must_run");
                        }
                        let what = if overrides.is_empty() {
                            "(no overrides)".to_string()
                        } else {
                            overrides.join(", ")
                        };
                        format!("  {name}  → {what}")
                    })
                    .collect();
                let select = match transport {
                    Transport::Cli => "`prova --profile <name>`",
                    Transport::Mcp => "`run { profile = \"<name>\" }`",
                };
                format!("**Profiles** (select with {select}):\n{}", rows.join("\n"))
            }
            Some(_) => "**Profiles**: none — `[profiles.<name>]` overlays `[run]` (CI is the \
                        usual first one)."
                .into(),
            None => String::new(),
        },
    }
}

/// Render a topic for a transport, substituting every slot from the environment. An unknown slot
/// is a bug caught by the in-crate tests; at runtime it renders as an explicit marker rather than
/// vanishing silently.
pub fn render(topic: Topic, env: &RenderEnv, transport: Transport) -> String {
    let mut out = String::new();
    let mut rest = topic.source();
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("}}") {
            Some(close) => {
                let name = after[..close].trim();
                match Slot::parse(name) {
                    Some(slot) => out.push_str(&render_slot(slot, env, transport)),
                    None => out.push_str(&format!("(unknown slot `{name}`)")),
                }
                rest = &after[close + 2..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The catalog listing: `key  hook` rows plus the transport-appropriate next move.
pub fn listing(transport: Transport) -> String {
    let width = Topic::ALL.iter().map(|t| t.key().len()).max().unwrap_or(0);
    let mut out = vec!["Topics — progressive disclosure, one screen each:".to_string(), String::new()];
    for topic in Topic::ALL {
        out.push(format!("  {:<width$}  {}", topic.key(), topic.hook()));
    }
    out.push(String::new());
    out.push(match transport {
        Transport::Cli => "Read one: `prova learn <topic>`.".to_string(),
        Transport::Mcp => "Read one: `learn { topic = \"<topic>\" }`.".to_string(),
    });
    out.join("\n")
}

/// `prova learn [<topic>]`.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut topic_arg: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("usage: prova learn [<topic>]\n\n{}", listing(Transport::Cli));
                return ExitCode::SUCCESS;
            }
            other if !other.starts_with('-') && topic_arg.is_none() => {
                topic_arg = Some(other.to_string());
            }
            other => {
                eprintln!("prova learn: unexpected argument {other:?}\nusage: prova learn [<topic>]");
                return ExitCode::from(2);
            }
        }
    }

    match topic_arg {
        None => {
            println!("{}", listing(Transport::Cli));
            ExitCode::SUCCESS
        }
        Some(name) => match Topic::resolve(&name) {
            Some(topic) => {
                let env = RenderEnv::at(Path::new("."));
                print!("{}", render(topic, &env, Transport::Cli));
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("prova learn: unknown topic {name:?}\n\n{}", listing(Transport::Cli));
                ExitCode::from(2)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enumerate every `{{slot}}` occurrence across all topics.
    fn slots_in(source: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = source;
        while let Some(open) = rest.find("{{") {
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else { break };
            out.push(after[..close].trim().to_string());
            rest = &after[close + 2..];
        }
        out
    }

    /// The slot vocabulary is CLOSED: every `{{name}}` a topic uses parses to a Slot variant.
    /// A typo'd or invented slot fails here, not silently at a user's terminal.
    #[test]
    fn every_slot_in_every_topic_is_in_the_vocabulary() {
        for topic in Topic::ALL {
            for name in slots_in(topic.source()) {
                assert!(
                    Slot::parse(&name).is_some(),
                    "topic `{}` uses unknown slot `{{{{{name}}}}}`",
                    topic.key()
                );
            }
        }
    }

    /// Every topic titles itself `# <key> — <hook>`: the listing hook is parsed from the title,
    /// so it is written once and cannot drift from the content.
    #[test]
    fn every_topic_titles_itself_with_its_key_and_hook() {
        for topic in Topic::ALL {
            let first = topic.source().lines().next().unwrap_or("");
            assert!(
                first.starts_with(&format!("# {} — ", topic.key())),
                "topic `{}` must start `# {} — <hook>`, got {first:?}",
                topic.key(),
                topic.key()
            );
            assert!(!topic.hook().is_empty(), "topic `{}` has an empty hook", topic.key());
        }
    }

    /// Aliases resolve, never collide with a key or each other, and every key resolves to itself.
    #[test]
    fn aliases_resolve_and_never_collide() {
        for topic in Topic::ALL {
            assert_eq!(Topic::resolve(topic.key()), Some(*topic));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (alias, target) in Topic::ALIASES {
            assert!(seen.insert(*alias), "alias {alias:?} appears twice");
            assert!(
                Topic::ALL.iter().all(|t| t.key() != *alias),
                "alias {alias:?} shadows a topic key"
            );
            assert_eq!(Topic::resolve(alias), Some(*target));
        }
        assert_eq!(Topic::resolve("mocks"), Some(Topic::Doubles));
        assert_eq!(Topic::resolve("no-such-topic"), None);
    }

    /// Every topic renders without a package (the degradation path) and stays one-screen-ish.
    #[test]
    fn every_topic_renders_without_a_package_and_stays_terse() {
        let env = RenderEnv { package: None, problem: None };
        for topic in Topic::ALL {
            for transport in [Transport::Cli, Transport::Mcp] {
                let text = render(*topic, &env, transport);
                assert!(!text.trim().is_empty(), "topic `{}` rendered empty", topic.key());
                assert!(
                    !text.contains("{{"),
                    "topic `{}` leaked an unrendered slot",
                    topic.key()
                );
                let lines = text.lines().count();
                assert!(
                    lines <= 90,
                    "topic `{}` is {lines} lines — split it (one screen per topic)",
                    topic.key()
                );
            }
        }
    }

    /// The listing carries every key and the transport-appropriate next move.
    #[test]
    fn listing_names_every_topic_and_the_next_move() {
        for transport in [Transport::Cli, Transport::Mcp] {
            let text = listing(transport);
            for topic in Topic::ALL {
                assert!(text.contains(topic.key()));
            }
        }
        assert!(listing(Transport::Cli).contains("prova learn <topic>"));
        assert!(listing(Transport::Mcp).contains("learn { topic"));
    }
}
