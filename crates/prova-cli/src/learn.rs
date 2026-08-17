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
use crate::manifest::{Manifest, PackageSource, Profile, Resolved};

/// Every topic the catalog serves. Adding a variant without a markdown file (or vice versa) is a
/// compile error; forgetting it in a match is too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Pdd,
    Promises,
    Reminders,
    Verifiers,
    Reports,
    Falsify,
    Spec,
    Claims,
    Backlog,
    Record,
    Evidence,
    Project,
    Init,
    Authoring,
    Fixtures,
    Doubles,
    Proxies,
    Drivers,
    Topologies,
    Packages,
    PackageAuthoring,
    Running,
    Locks,
    Capabilities,
    Mcp,
}

impl Topic {
    pub const ALL: &'static [Topic] = &[
        Topic::Pdd,
        Topic::Promises,
        Topic::Reminders,
        Topic::Verifiers,
        Topic::Reports,
        Topic::Falsify,
        Topic::Spec,
        Topic::Claims,
        Topic::Backlog,
        Topic::Record,
        Topic::Evidence,
        Topic::Project,
        Topic::Init,
        Topic::Authoring,
        Topic::Fixtures,
        Topic::Doubles,
        Topic::Proxies,
        Topic::Drivers,
        Topic::Topologies,
        Topic::Packages,
        Topic::PackageAuthoring,
        Topic::Running,
        Topic::Locks,
        Topic::Capabilities,
        Topic::Mcp,
    ];

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
        ("specs", Topic::Spec),
        ("burndown", Topic::Promises),
        ("xfail", Topic::Promises),
        ("pending", Topic::Promises),
        // The switches primitive is taught where selection is taught — the running topic's
        // opt-in-classes section (docs/design/manifest.md#switches-not-env-capabilities).
        ("switches", Topic::Running),
        ("switch", Topic::Running),
        ("falsifier", Topic::Falsify),
        // The conduct janitor (`prova reap`) is taught where conduct supervision is taught.
        ("reap", Topic::Verifiers),
        ("reaper", Topic::Verifiers),
        ("falsifiers", Topic::Falsify),
        ("falsified_by", Topic::Falsify),
        ("vacuous", Topic::Falsify),
        ("mutation", Topic::Falsify),
        ("owed", Topic::Claims),
        ("covers", Topic::Claims),
        ("anchor", Topic::Claims),
        ("drift", Topic::Claims),
        ("attest", Topic::Record),
        ("ledger", Topic::Evidence),
        ("account", Topic::Evidence),
        ("lifecycle", Topic::Evidence),
        ("promise", Topic::Promises),
        ("reminder", Topic::Reminders),
        ("remind", Topic::Reminders),
        ("attention", Topic::Reminders),
        ("due", Topic::Reminders),
        ("tripwire", Topic::Reminders),
        ("reverse-checklist", Topic::Reminders),
        ("verifier", Topic::Verifiers),
        ("junit", Topic::Verifiers),
        ("deputed", Topic::Verifiers),
        ("report", Topic::Reports),
        ("custody", Topic::Reports),
        ("artifact", Topic::Reports),
        ("lanes", Topic::Verifiers),
        ("lane", Topic::Verifiers),
        ("run-record", Topic::Record),
        ("skipped", Topic::Record),
        ("deselected", Topic::Record),
        ("tests", Topic::Authoring),
        ("dsl", Topic::Authoring),
        ("matchers", Topic::Authoring),
        ("introspect", Topic::Authoring),
        ("api", Topic::Authoring),
        ("snapshots", Topic::Authoring),
        ("fixture", Topic::Fixtures),
        ("scopes", Topic::Fixtures),
        ("proxy", Topic::Proxies),
        ("driver", Topic::Drivers),
        ("protocols", Topic::Drivers),
        ("topology", Topic::Topologies),
        ("plugin", Topic::Packages),
        ("plugins", Topic::Packages),
        ("authoring-packages", Topic::PackageAuthoring),
        ("authoring-plugins", Topic::PackageAuthoring),
        ("plugin-authoring", Topic::PackageAuthoring),
        ("create-package", Topic::PackageAuthoring),
        ("create-plugin", Topic::PackageAuthoring),
        ("lock", Topic::Locks),
        ("locking", Topic::Locks),
        ("serial", Topic::Locks),
        ("serialization", Topic::Locks),
        ("resources", Topic::Locks),
        ("scheduler", Topic::Locks),
        ("selection", Topic::Running),
        ("ci", Topic::Running),
        ("cli", Topic::Running),
        ("warm", Topic::Mcp),
        ("server", Topic::Mcp),
        // Command-keyword resolution: every verb teaches itself. These route a VERB word to the
        // doctrine topic that explains it, so `prova learn <verb>` never dead-ends — principled (the
        // word you typed as a command resolves), categorically unlike a concept synonym. Held by the
        // `every_verb_resolves_in_learn` proof; new verbs that skip a home fail it.
        ("run", Topic::Running),
        ("eval", Topic::Running),
        ("list", Topic::Running),
        ("ide", Topic::Init),
        ("skill", Topic::Pdd),
        ("up", Topic::Topologies),
        ("down", Topic::Topologies),
        ("start", Topic::Topologies),
        ("watch", Topic::Topologies),
        ("ps", Topic::Topologies),
        ("broker", Topic::Topologies),
    ];

    pub fn key(self) -> &'static str {
        match self {
            Topic::Pdd => "pdd",
            Topic::Promises => "promises",
            Topic::Reminders => "reminders",
            Topic::Verifiers => "verifiers",
            Topic::Falsify => "falsify",
            Topic::Spec => "spec",
            Topic::Claims => "claims",
            Topic::Backlog => "backlog",
            Topic::Reports => "reports",
            Topic::Record => "record",
            Topic::Evidence => "evidence",
            Topic::Project => "project",
            Topic::Init => "init",
            Topic::Authoring => "authoring",
            Topic::Fixtures => "fixtures",
            Topic::Doubles => "doubles",
            Topic::Proxies => "proxies",
            Topic::Drivers => "drivers",
            Topic::Topologies => "topologies",
            Topic::Packages => "packages",
            Topic::PackageAuthoring => "package-authoring",
            Topic::Running => "running",
            Topic::Locks => "locks",
            Topic::Capabilities => "capabilities",
            Topic::Mcp => "mcp",
        }
    }

    /// The embedded doctrine. One file per variant; the pairing is what makes an undocumented
    /// topic unrepresentable.
    fn source(self) -> &'static str {
        match self {
            Topic::Pdd => include_str!("topics/pdd.md"),
            Topic::Promises => include_str!("topics/promises.md"),
            Topic::Reminders => include_str!("topics/reminders.md"),
            Topic::Verifiers => include_str!("topics/verifiers.md"),
            Topic::Falsify => include_str!("topics/falsify.md"),
            Topic::Spec => include_str!("topics/spec.md"),
            Topic::Claims => include_str!("topics/claims.md"),
            Topic::Backlog => include_str!("topics/backlog.md"),
            Topic::Reports => include_str!("topics/reports.md"),
            Topic::Record => include_str!("topics/record.md"),
            Topic::Evidence => include_str!("topics/evidence.md"),
            Topic::Project => include_str!("topics/project.md"),
            Topic::Init => include_str!("topics/init.md"),
            Topic::Authoring => include_str!("topics/authoring.md"),
            Topic::Fixtures => include_str!("topics/fixtures.md"),
            Topic::Doubles => include_str!("topics/doubles.md"),
            Topic::Proxies => include_str!("topics/proxies.md"),
            Topic::Drivers => include_str!("topics/drivers.md"),
            Topic::Topologies => include_str!("topics/topologies.md"),
            Topic::Packages => include_str!("topics/packages.md"),
            Topic::PackageAuthoring => include_str!("topics/package-authoring.md"),
            Topic::Running => include_str!("topics/running.md"),
            Topic::Locks => include_str!("topics/locks.md"),
            Topic::Capabilities => include_str!("topics/capabilities.md"),
            Topic::Mcp => include_str!("topics/mcp.md"),
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
    Agent,
    ProofPaths,
    PackagesDir,
    Packages,
    Registries,
    Topologies,
    Profiles,
    /// `[[specs.source]]` — where prose obligations live, and where a new one should be written.
    Specs,
    /// The opt-in classes thrown by config, with the pointer at `prova switches` for the live set.
    Switches,
    /// Where prova's own files live: the manifest variant, the config companion, the state dir.
    Artifacts,
    ContextFiles,
}

impl Slot {
    fn parse(name: &str) -> Option<Slot> {
        match name {
            "init_catalog" => Some(Slot::InitCatalog),
            "agent" => Some(Slot::Agent),
            "proof_paths" => Some(Slot::ProofPaths),
            "packages_dir" => Some(Slot::PackagesDir),
            "packages" => Some(Slot::Packages),
            "registries" => Some(Slot::Registries),
            "topologies" => Some(Slot::Topologies),
            "profiles" => Some(Slot::Profiles),
            "specs" => Some(Slot::Specs),
            "switches" => Some(Slot::Switches),
            "artifacts" => Some(Slot::Artifacts),
            "context_files" => Some(Slot::ContextFiles),
            _ => None,
        }
    }
}

/// The package the renderer computes dynamic facts from — resolved fresh per render, so the
/// answer is true at the moment of asking.
struct PackageFacts {
    manifest_name: String,
    home_dir: std::path::PathBuf,
    /// The directory the manifest lives in — the `.prova/` nook, or the root when the manifest is
    /// flat. Where a `CONTEXT.md` is looked for (a sibling of `prova.toml`).
    nook_dir: std::path::PathBuf,
    resolved: Resolved,
    profiles: BTreeMap<String, Profile>,
    /// `[[specs.source]]` declarations — the prose layer's roots (empty when the package never
    /// opted in).
    specs: Vec<crate::manifest::SpecSource>,
    /// `[run] switches` — the classes the package baseline itself throws.
    run_switches: Vec<String>,
    /// `[agent] spec_first` (default on) — drives the `{{agent}}` nudge in the project topic.
    spec_first: bool,
}


/// The package's **local** plugins: the requirable subdirectories of `[run] plugin_root`.
///
/// These are plugins in every sense that matters to an author — `require("<name>")` resolves them —
/// but they appear in no manifest table, so anything that reads `[plugins]` alone reports a package
/// full of them as having none. A directory counts when it holds an `init.lua`, which is exactly what
/// the resolver requires.
fn local_packages(p: &PackageFacts) -> Vec<String> {
    local_packages_in(&p.home_dir, p.resolved.packages_dir.as_deref())
}

/// The scan itself, split out so it is testable without a resolved manifest.
fn local_packages_in(home_dir: &std::path::Path, plugin_root: Option<&str>) -> Vec<String> {
    let Some(root) = plugin_root else {
        return Vec::new();
    };
    let dir = home_dir.join(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("init.lua").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// One project-provided context doc (manifest `context`), surfaced as a `ctx:<stem>` topic.
pub struct ContextDoc {
    /// The listing key: `ctx:<file stem>`.
    pub key: String,
    /// The declared (home-relative or `~/`) path, for error messages.
    pub declared: String,
    /// The resolved absolute path.
    pub path: std::path::PathBuf,
}

impl ContextDoc {
    /// The listing hook: the file's first heading/line, or a LOUD missing marker — a declared
    /// doc is never silently absent.
    pub fn hook(&self) -> String {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => text
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim_start_matches(['#', ' '])
                .to_string(),
            Err(_) => format!("(missing: {})", self.declared),
        }
    }
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
                    home_dir: home.dir.clone(),
                    nook_dir: home
                        .manifest
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| home.dir.clone()),
                    resolved,
                    specs: m.specs.as_ref().map(|s| s.source.clone()).unwrap_or_default(),
                    run_switches: m.run.switches.clone(),
                    profiles: m.profiles,
                    spec_first: m.agent.spec_first(),
                })
            });
        match load {
            Ok(p) => RenderEnv { package: Some(p), problem: None },
            Err(e) => RenderEnv { package: None, problem: Some(e) },
        }
    }

    /// The package's declared context docs, `~/` expanded and home-relative paths anchored.
    pub fn context_docs(&self) -> Vec<ContextDoc> {
        let Some(p) = &self.package else { return Vec::new() };
        p.resolved
            .context
            .iter()
            .map(|declared| {
                let path = match declared.strip_prefix("~/") {
                    Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                    None => p.home_dir.join(declared),
                };
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| declared.clone());
                ContextDoc { key: format!("ctx:{stem}"), declared: declared.clone(), path }
            })
            .collect()
    }

    fn no_package_line(&self, transport: Transport) -> String {
        if let Some(problem) = &self.problem {
            return format!("A manifest was found but could not be loaded: {problem}");
        }
        match transport {
            Transport::Cli => "No prova.toml found from the working directory — run `prova init` \
                               to scaffold a package (see `prova learn init`), or run from \
                               inside one."
                .into(),
            Transport::Mcp => "No prova.toml found from the server's working directory — pass \
                               `package = \"<dir>\"` on this call to point at one, or shell out \
                               to `prova init` to scaffold (no MCP tool scaffolds; see \
                               `learn { topic = \"init\" }`)."
                .into(),
        }
    }
}

/// One plugin source, described the way an agent would re-declare it.
fn describe_source(source: &PackageSource) -> String {
    match source {
        PackageSource::Path(s) => s.clone(),
        PackageSource::Detailed(d) => {
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

/// The `prova init` catalog listing, with the transport-appropriate render hint.
fn render_init_catalog(transport: Transport) -> String {
    {
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
}

/// The package vocabulary: local packages and `[dependencies]`, as one `require` list.
fn render_packages(env: &RenderEnv) -> String {
    match &env.package {
        Some(p)
            if !p.resolved.dependencies.is_empty() || !local_packages(p).is_empty() =>
        {
            // BOTH kinds, because `require("<name>")` does not distinguish them. Listing only
            // the `[plugins]` table told a package with three working local plugins that it had
            // "none" — a true statement about one manifest key, and a false answer to the
            // question actually being asked ("what vocabulary do I have here?").
            let local = local_packages(p);
            let width = p
                .resolved
                .dependencies
                .keys()
                .chain(local.iter())
                .map(String::len)
                .max()
                .unwrap_or(0);
            let root = p.resolved.packages_dir.as_deref().unwrap_or("");
            let rows: Vec<String> = local
                .iter()
                .map(|name| format!("  {name:<width$}  local ({root}/{name})"))
                .chain(
                    p.resolved
                        .dependencies
                        .iter()
                        .map(|(name, src)| format!("  {name:<width$}  {}", describe_source(src))),
                )
                .collect();
            format!(
                "**Packages** (`require(\"<name>\")` in any proof):\n{}",
                rows.join("\n")
            )
        }
        Some(_) => "**Packages**: none — declare external ones under `[dependencies]` in the \
                    manifest, or author local ones under `[run] packages`."
            .into(),
        // The long no-package guidance renders once, on the ProofPaths slot; here one short
        // line keeps a doctrine topic readable outside a package.
        None => "(no package in reach — declared dependencies unknown)".into(),
    }
}

/// The declared `[topologies]`, with the transport-appropriate verb to hold one.
fn render_topologies(env: &RenderEnv, transport: Transport) -> String {
    match &env.package {
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
                    format!("  {name}  → package `{}` {what}{requires}", t.package)
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
        // Same one-liner as the Plugins slot: this slot sits under an "## In this package"
        // heading in topologies.md, and a heading over nothing reads as a rendering bug.
        None => "(no package in reach — declared topologies unknown)".into(),
    }
}

/// Project context: an inlined `CONTEXT.md` and the declared `context = [...]` docs.
fn render_context_files(env: &RenderEnv) -> String {
    match &env.package {
        Some(p) => {
            let mut out = String::new();
            // `<nook>/CONTEXT.md` — a zero-config project brief, inlined verbatim. Drop the file
            // and it rides `prova learn project`; no manifest entry needed. This is the project's
            // own words to an agent orienting here (team conventions, gotchas, where to start).
            //
            // Looked for beside the manifest first, then in `<root>/.prova/` — a FLAT manifest
            // (`.prova.toml` at the root) very often still keeps a `.prova/` state nook, and the
            // brief belongs tucked there with prova's other files, not as one more root file.
            let context_md = std::fs::read_to_string(p.nook_dir.join("CONTEXT.md"))
                .or_else(|_| std::fs::read_to_string(p.home_dir.join(".prova/CONTEXT.md")));
            if let Ok(md) = context_md {
                let md = md.trim_end();
                if !md.is_empty() {
                    out.push_str("## Project context (`CONTEXT.md`)\n\n");
                    out.push_str(md);
                    out.push_str("\n\n");
                }
            }
            // The declared `context = [...]` docs, as `ctx:<stem>` pointers (read on demand).
            let docs = env.context_docs();
            if docs.is_empty() {
                if out.is_empty() {
                    out.push_str(
                        "**Project context**: none — drop a `CONTEXT.md` beside the manifest (or \
                         in `.prova/`; inlined here), or declare `context = [\"docs/agent.md\"]` \
                         for `ctx:<stem>` topics.",
                    );
                }
            } else {
                let rows: Vec<String> =
                    docs.iter().map(|d| format!("  {}  {}", d.key, d.hook())).collect();
                out.push_str(&format!(
                    "**Project context** (read with `prova learn ctx:<stem>`):\n{}",
                    rows.join("\n")
                ));
            }
            out.trim_end().to_string()
        }
        None => String::new(),
    }
}

/// The `[profiles.*]` lanes: description first, then the facts an agent keys on.
fn render_profiles(env: &RenderEnv, transport: Transport) -> String {
    match &env.package {
        Some(p) if !p.profiles.is_empty() => {
            // Rich rows, so "which profile, when?" is answered here: the author's description
            // first, then the facts an agent keys on — what it selects, which switches it
            // throws, what it guarantees.
            let rows: Vec<String> = p
                .profiles
                .iter()
                .map(|(name, profile)| {
                    let mut chips: Vec<String> = Vec::new();
                    if !profile.proofs.is_empty() {
                        chips.push(format!("selects: {}", profile.proofs.join(", ")));
                    }
                    if !profile.tags.is_empty() {
                        chips.push(format!("tags: {}", profile.tags.join(", ")));
                    }
                    if !profile.switches.is_empty() {
                        chips.push(format!("throws: {}", profile.switches.join(", ")));
                    }
                    if !profile.must_run.is_empty() {
                        chips.push(format!("guarantees: {}", profile.must_run.join(", ")));
                    }
                    if !profile.env.is_empty() {
                        chips.push("env".to_string());
                    }
                    if !profile.dependencies.is_empty() {
                        chips.push("dependencies".to_string());
                    }
                    let what = if chips.is_empty() {
                        "(no overrides)".to_string()
                    } else {
                        chips.join(" · ")
                    };
                    match &profile.description {
                        Some(d) => format!("  {name}  — {d}\n           {what}"),
                        None => format!("  {name}  — {what}"),
                    }
                })
                .collect();
            let select = match transport {
                Transport::Cli => "`prova run <name>`",
                Transport::Mcp => "`run { profile = \"<name>\" }`",
            };
            format!("**Profiles** (run with {select}):\n{}", rows.join("\n"))
        }
        Some(_) => "**Profiles**: none — `[profiles.<name>]` overlays `[run]` (CI is the \
                    usual first one)."
            .into(),
        None => String::new(),
    }
}

/// The opt-in classes config throws; the live inventory is `prova switches`' job.
fn render_switches(env: &RenderEnv) -> String {
    match &env.package {
        Some(p) => {
            // The classes CONFIG throws (statically knowable); the live class inventory needs
            // collection, which is `prova switches`' job — the card points, never loads.
            let mut thrown: Vec<String> = Vec::new();
            if !p.run_switches.is_empty() {
                thrown.push(format!("{} ([run] — every run)", p.run_switches.join(", ")));
            }
            for (name, profile) in &p.profiles {
                if !profile.switches.is_empty() {
                    thrown.push(format!(
                        "{} (profile `{name}`)",
                        profile.switches.join(", ")
                    ));
                }
            }
            let lead = "**Switches** (opt-in classes: `switch = \"<class>\"` is off unless \
                        thrown with `-s <class>` or a profile)";
            if thrown.is_empty() {
                format!(
                    "{lead}: none thrown by config — `prova switches` lists every declared \
                     class and who throws it."
                )
            } else {
                format!(
                    "{lead}: thrown by config: {} — `prova switches` lists every declared \
                     class and who throws it.",
                    thrown.join("; ")
                )
            }
        }
        None => String::new(),
    }
}

fn render_slot(slot: Slot, env: &RenderEnv, transport: Transport) -> String {
    match slot {
        Slot::InitCatalog => render_init_catalog(transport),
        Slot::ProofPaths => match &env.package {
            Some(p) => format!(
                "**Proofs** ({}): `proofs = [{}]` — directory-NAME patterns; every matching \
                 directory below the package root holds `*.prova.lua` proofs (`*_test.lua` is the \
                 accepted older spelling). Put new proofs there.",
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
        Slot::PackagesDir => match &env.package {
            Some(p) => match &p.resolved.packages_dir {
                Some(root) => format!(
                    "**Local packages**: author them under `{root}/<name>/` (the declared \
                     `packages` directory); each subdirectory is requirable by name."
                ),
                None => "**Local packages**: no `packages` directory declared — set \
                         `[run] packages` in the manifest before authoring package-local packages."
                    .into(),
            },
            None => String::new(),
        },
        // No package needed and no fetch performed: registries come from user config alone, so
        // this renders (and stays truthful) offline and pre-init.
        Slot::Registries => crate::registry::learn_lines(transport == Transport::Cli),
        Slot::Packages => render_packages(env),
        Slot::Topologies => render_topologies(env, transport),
        Slot::ContextFiles => render_context_files(env),
        Slot::Agent => match &env.package {
            // The spec-first nudge — on by default (`[agent] spec_first = false` silences it). Kept to
            // an inclination, not a rule: prefer authoring behaviour as `spec`-flagged proofs over a
            // prose design doc, and burn the backlog down. When off, the slot is empty.
            Some(p) if p.spec_first => "**Spec-first here.** Prefer capturing new behaviour as a \
                `promises`-flagged proof (the proof *is* the contract — no prose doc to drift) over a \
                design doc: `prova tests --promises` lists the open surface, `prova tests burndown` implements it, \
                graduate `promises = \"…\"` to `proves = \"…\"` when green. (`[agent] spec_first = false` to silence.)"
                .into(),
            _ => String::new(),
        },
        Slot::Profiles => render_profiles(env, transport),
        Slot::Specs => match &env.package {
            Some(p) if !p.specs.is_empty() => {
                let roots: Vec<String> = p
                    .specs
                    .iter()
                    .map(|s| match s {
                        crate::manifest::SpecSource::Directory { path } => {
                            format!("`{path}` (directory, writable)")
                        }
                    })
                    .collect();
                format!(
                    "**Specs** (`[[specs.source]]`): {} — write a new `<!-- claim: id -->` or \
                     `<!-- backlog: id -->` anchor in the doc whose subject fits (create one under \
                     a writable source if none does). `prova specs` lists the lane; `prova owed` \
                     says what is still unproven.",
                    roots.join(", ")
                )
            }
            Some(_) => "**Specs**: none declared — `[[specs.source]]` opts the prose layer in \
                        (`prova learn spec`); until then claims and backlog have no home."
                .into(),
            None => String::new(),
        },
        Slot::Switches => render_switches(env),
        Slot::Artifacts => match &env.package {
            Some(p) => {
                let config = p
                    .resolved
                    .config
                    .as_deref()
                    .unwrap_or("prova.lua (beside the manifest, if present)");
                format!(
                    "**Prova's own files**: manifest `{}` · Lua companion `{config}` \
                     (`runtime.capability` lives there) · state under `.prova/var/` at the root \
                     (run records, baselines — machine-local, never committed). Host check: \
                     `prova capabilities` reports the built-in vocabulary MET/UNMET on this box.",
                    p.manifest_name
                )
            }
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
    // Empty slot renders leave runs of blank lines behind — collapse them so a degraded topic
    // reads clean, not gappy.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

/// The catalog listing: `key  hook` rows (plus this package's `ctx:*` docs) and the
/// transport-appropriate next move.
pub fn listing(env: &RenderEnv, transport: Transport) -> String {
    let context = env.context_docs();
    let width = Topic::ALL
        .iter()
        .map(|t| t.key().len())
        .chain(context.iter().map(|d| d.key.len()))
        .max()
        .unwrap_or(0);
    let mut out = vec!["Topics — progressive disclosure, one screen each:".to_string(), String::new()];
    for topic in Topic::ALL {
        out.push(format!("  {:<width$}  {}", topic.key(), topic.hook()));
    }
    if !context.is_empty() {
        out.push(String::new());
        out.push("Project context (this package's own docs, from prova.toml `context`):".into());
        for doc in &context {
            out.push(format!("  {:<width$}  {}", doc.key, doc.hook()));
        }
    }
    out.push(String::new());
    out.push(match transport {
        Transport::Cli => "Read one: `prova learn <topic>`.".to_string(),
        Transport::Mcp => "Read one: `learn { topic = \"<topic>\" }`.".to_string(),
    });
    out.join("\n")
}

/// Answer a `learn` ask — the ONE path every surface (CLI, MCP tool, MCP resources) goes
/// through, so they cannot disagree. `Err` is the usage-error text (unknown topic, unreadable
/// context doc); the caller decides exit code vs error result.
pub fn answer(topic: Option<&str>, env: &RenderEnv, transport: Transport) -> Result<String, String> {
    let name = match topic.map(str::trim) {
        None | Some("") => return Ok(listing(env, transport)),
        Some(name) => name,
    };
    if let Some(topic) = Topic::resolve(name) {
        return Ok(render(topic, env, transport));
    }
    let needle = name.to_lowercase();
    if let Some(doc) = env.context_docs().into_iter().find(|d| d.key == needle) {
        return std::fs::read_to_string(&doc.path).map_err(|e| {
            format!(
                "context doc {} (declared in prova.toml `context` as {:?}) cannot be read: {e}",
                doc.path.display(),
                doc.declared
            )
        });
    }
    Err(format!("unknown topic {name:?}\n\n{}", listing(env, transport)))
}

/// `prova learn [<topic>]`.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut topic_arg: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                let env = RenderEnv::at(Path::new("."));
                println!("usage: prova learn [<topic>]\n\n{}", listing(&env, Transport::Cli));
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

    let env = RenderEnv::at(Path::new("."));
    match answer(topic_arg.as_deref(), &env, Transport::Cli) {
        Ok(text) => {
            println!("{}", text.trim_end());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("prova learn: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// Local plugins are listed from `plugin_root`, because `require("<name>")` resolves them and an
    /// author asking "what do I have here?" means both kinds. A directory without `init.lua` is not a
    /// plugin, and neither is a stray file.
    #[test]
    fn local_plugins_are_found_under_the_plugin_root() {
        let base = std::env::temp_dir().join(format!("prova-learn-{}", std::process::id()));
        let root = base.join(".prova/plugins");
        for name in ["minion", "policy"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("init.lua"), "return {}").unwrap();
        }
        std::fs::create_dir_all(root.join("not-a-plugin")).unwrap(); // no init.lua
        std::fs::write(root.join("README.md"), "# plugins").unwrap(); // not a directory

        assert_eq!(
            local_packages_in(&base, Some(".prova/plugins")),
            vec!["minion".to_string(), "policy".to_string()]
        );
        // No `plugin_root` declared, or one that does not exist: no plugins, no error.
        assert!(local_packages_in(&base, None).is_empty());
        assert!(local_packages_in(&base, Some("nope")).is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

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

    /// The `[agent] spec_first` nudge (default on, opt-out) and the zero-config `CONTEXT.md` inline —
    /// the configurable-skill seed: `learn project` content is a function of project config + state.
    #[test]
    fn agent_nudge_and_context_md_render_in_project() {
        let base = std::env::temp_dir().join(format!("prova-agent-{}", std::process::id()));
        let nook = base.join(".prova");
        std::fs::create_dir_all(nook.join("proofs").parent().unwrap()).unwrap();
        std::fs::create_dir_all(&nook).unwrap();
        let manifest = |extra: &str| format!("[run]\nproofs = [\"proofs\"]\n{extra}");

        // Default: spec_first on → the nudge renders.
        std::fs::write(nook.join("prova.toml"), manifest("")).unwrap();
        let env = RenderEnv::at(&base);
        assert!(
            render_slot(Slot::Agent, &env, Transport::Cli).contains("Spec-first"),
            "spec_first defaults on"
        );

        // Opt out → the slot is empty (silent, not a message).
        std::fs::write(nook.join("prova.toml"), manifest("[agent]\nspec_first = false\n")).unwrap();
        assert_eq!(render_slot(Slot::Agent, &RenderEnv::at(&base), Transport::Cli), "");

        // A `.prova/CONTEXT.md` is inlined verbatim into the context slot.
        std::fs::write(nook.join("prova.toml"), manifest("")).unwrap();
        std::fs::write(nook.join("CONTEXT.md"), "# Orient\n\nStart at proofs/.").unwrap();
        let ctx = render_slot(Slot::ContextFiles, &RenderEnv::at(&base), Transport::Cli);
        assert!(ctx.contains("CONTEXT.md") && ctx.contains("Start at proofs/."), "inlined: {ctx}");

        std::fs::remove_dir_all(&base).ok();
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
                assert!(
                    !text.contains("\n\n\n"),
                    "topic `{}` renders gappy (empty slots must collapse)",
                    topic.key()
                );
                assert!(
                    text.matches("No prova.toml found").count() <= 1,
                    "topic `{}` repeats the no-package guidance",
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
        let env = RenderEnv { package: None, problem: None };
        for transport in [Transport::Cli, Transport::Mcp] {
            let text = listing(&env, transport);
            for topic in Topic::ALL {
                assert!(text.contains(topic.key()));
            }
        }
        assert!(listing(&env, Transport::Cli).contains("prova learn <topic>"));
        assert!(listing(&env, Transport::Mcp).contains("learn { topic"));
    }

    /// `answer` is the one path every surface shares: listing, topic, alias, unknown.
    #[test]
    fn answer_routes_listing_topic_and_unknown() {
        let env = RenderEnv { package: None, problem: None };
        assert!(answer(None, &env, Transport::Cli).unwrap().contains("doubles"));
        assert!(answer(Some("mocks"), &env, Transport::Cli).unwrap().contains("http.mock"));
        let err = answer(Some("nope"), &env, Transport::Cli).unwrap_err();
        assert!(err.contains("unknown topic"));
        assert!(err.contains("pdd"), "the listing rides the error");
        // Outside a package there are no ctx topics.
        assert!(answer(Some("ctx:anything"), &env, Transport::Cli).is_err());
    }
}
