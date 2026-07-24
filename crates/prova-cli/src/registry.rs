//! The plugin registry: discovery across config-listed registries (docs/design/registry.md).
//!
//! A registry is a git repository (or a local directory — same source classification as plugin
//! sources) holding one TOML entry per plugin under `registry/`. Strictly discovery-only: nothing
//! here participates in require-time resolution. `prova plugins` lists/searches, `info` details,
//! and `add` materializes an ordinary pinned `[plugins]` entry into `prova.toml` — from that
//! moment the registry is out of the picture and the committed manifest tells the whole story.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use prova_core::{SystemLayout, XdgSystemLayout};
use serde::Deserialize;

use crate::home;
use crate::manifest::PluginDetail;
use crate::plugins::{is_git_source, GitFetchOptions};

/// The registry entry schema this binary understands. Entries carrying another major are skipped
/// per-entry with a warning — old binary, newer registry: degraded, never broken.
const KNOWN_SCHEMA: i64 = 1;

/// One configured registry: a trust-granularity name and a source (git URL or local path).
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryRef {
    pub name: String,
    pub source: String,
}

/// The `[[registries]]` section of `~/.config/prova/config.toml`. Unknown tables are ignored so
/// this parser and the init catalog's can each claim their section of the same file.
#[derive(Debug, Deserialize, Default)]
struct RegistriesConfig {
    #[serde(default)]
    registries: Vec<RegistryRef>,
}

/// The registries prova ships with. `prova-rs` is present unconditionally — the same rule that
/// makes `prova init` work on a machine with no config at all.
fn builtin() -> Vec<RegistryRef> {
    vec![RegistryRef {
        name: "prova-rs".to_string(),
        source: "https://github.com/prova-rs/package-registry".to_string(),
    }]
}

/// Built-ins merged with `<config_dir>/config.toml` `[[registries]]`: a user entry whose name
/// matches a built-in replaces it wholesale; a new name adds. Missing config is normal; an
/// unreadable or malformed one is an error naming the file (the init catalog's rule).
pub fn configured(layout: &dyn SystemLayout) -> Result<Vec<RegistryRef>, String> {
    let mut regs = builtin();
    let path = layout.config_dir().join("config.toml");
    if !path.is_file() {
        return Ok(regs);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let user: RegistriesConfig =
        toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    for r in user.registries {
        match regs.iter_mut().find(|e| e.name == r.name) {
            Some(existing) => *existing = r,
            None => regs.push(r),
        }
    }
    Ok(regs)
}

/// One plugin entry, as served. `registry` is the serving registry's configured name — shown in
/// listings when more than one registry is configured, and the disambiguator for `add`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub registry: String,
    pub name: String,
    pub repo: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub latest: Option<String>,
    pub namespaces: Vec<String>,
    pub topologies: Vec<String>,
    pub shapes: Vec<String>,
    pub requires: Vec<String>,
}

/// The on-disk shape, parsed leniently: every field optional, unknown keys ignored (graceful
/// extensibility — an entry can grow fields without breaking older binaries). Requiredness is
/// validated after parse so a miss is a per-entry warning, never a registry-wide failure.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct EntryFile {
    schema: Option<i64>,
    name: Option<String>,
    repo: Option<String>,
    description: Option<String>,
    capabilities: Vec<String>,
    latest: Option<String>,
    namespaces: Vec<String>,
    topologies: Vec<String>,
    shapes: Vec<String>,
    requires: Vec<String>,
}

/// Resolve a registry source to a local directory. A git source goes through the same
/// content-addressed, freshness-gated cache as plugin sources (the checkout IS the cache — no
/// secondary index); anything else is a local path. The lease keeps a fetched tree from the
/// pruner while it is being read.
fn registry_dir(
    reg: &RegistryRef,
    layout: &dyn SystemLayout,
    git_opts: &GitFetchOptions,
) -> Result<(PathBuf, Option<archetect_git_cache::Lease>), String> {
    if is_git_source(&reg.source) {
        let detail = PluginDetail {
            path: None,
            git: Some(reg.source.clone()),
            tag: None,
            branch: None,
            rev: None,
            module: None,
        };
        let (dir, lease) = crate::plugins::fetch_git(&reg.source, &detail, layout, git_opts)
            .map_err(|e| format!("registry {}: {e}", reg.name))?;
        return Ok((dir, Some(lease)));
    }
    let dir = PathBuf::from(&reg.source);
    if !dir.is_dir() {
        return Err(format!(
            "registry {}: source {} is not a directory",
            reg.name, reg.source
        ));
    }
    Ok((dir, None))
}

/// Read every `registry/*.toml` under a resolved registry dir. Tolerance is the contract: an
/// entry with an unknown schema major, a missing required field, or unparseable TOML is skipped
/// with a warning naming it; its siblings still serve.
fn load_entries(reg: &RegistryRef, dir: &Path, warnings: &mut Vec<String>) -> Vec<Entry> {
    let entries_dir = dir.join("registry");
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&entries_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
            .collect(),
        Err(e) => {
            warnings.push(format!(
                "registry {}: cannot read {}: {e}",
                reg.name,
                entries_dir.display()
            ));
            return Vec::new();
        }
    };
    files.sort();

    let mut out = Vec::new();
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<entry>")
            .to_string();
        let skip = |warnings: &mut Vec<String>, why: String| {
            warnings.push(format!("registry {}: skipping entry {stem}: {why}", reg.name));
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                skip(&mut *warnings, format!("cannot read: {e}"));
                continue;
            }
        };
        let file: EntryFile = match toml::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                skip(&mut *warnings, format!("invalid TOML: {e}"));
                continue;
            }
        };
        let schema = file.schema.unwrap_or(KNOWN_SCHEMA);
        if schema != KNOWN_SCHEMA {
            skip(
                &mut *warnings,
                format!("schema {schema} is newer than this binary understands ({KNOWN_SCHEMA})"),
            );
            continue;
        }
        let (Some(name), Some(repo), Some(description)) =
            (file.name, file.repo, file.description)
        else {
            skip(
                &mut *warnings,
                "missing a required field (name, repo, description)".to_string(),
            );
            continue;
        };
        out.push(Entry {
            registry: reg.name.clone(),
            name,
            repo,
            description,
            capabilities: file.capabilities,
            latest: file.latest,
            namespaces: file.namespaces,
            topologies: file.topologies,
            shapes: file.shapes,
            requires: file.requires,
        });
    }
    out
}

/// Everything the configured registries currently serve, plus what went wrong along the way.
struct Loaded {
    entries: Vec<Entry>,
    /// Per-entry skips (tolerance): warn and keep serving siblings.
    warnings: Vec<String>,
    /// Whole-registry failures (unreachable offline, bad path): shown, and they fail the command.
    errors: Vec<String>,
    registry_count: usize,
}

/// Load every configured registry. Per-entry problems are warnings; a registry that cannot be
/// served at all is an error — returned alongside whatever did load, so the caller can both
/// show the working rows and fail loud.
fn load_all(layout: &dyn SystemLayout, git_opts: &GitFetchOptions) -> Result<Loaded, String> {
    let regs = configured(layout)?;
    let registry_count = regs.len();
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for reg in &regs {
        match registry_dir(reg, layout, git_opts) {
            Ok((dir, _lease)) => entries.extend(load_entries(reg, &dir, &mut warnings)),
            Err(e) => errors.push(e),
        }
    }
    entries.sort_by(|a, b| (&a.name, &a.registry).cmp(&(&b.name, &b.registry)));
    Ok(Loaded { entries, warnings, errors, registry_count })
}

/// Whether an entry matches a search term: substring over name, description, and capabilities,
/// case-insensitive. A few hundred entries in memory — no index, no query language.
fn matches(e: &Entry, q: &str) -> bool {
    let q = q.to_lowercase();
    e.name.to_lowercase().contains(&q)
        || e.description.to_lowercase().contains(&q)
        || e.capabilities.iter().any(|c| c.to_lowercase().contains(&q))
}

/// Rows on stdout so it pipes, key-column aligned like the init catalog; the serving registry is
/// shown whenever more than one is configured (it is the `add` disambiguator).
fn print_rows(entries: &[Entry], multi: bool) {
    let width = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let rwidth = entries.iter().map(|e| e.registry.len()).max().unwrap_or(0);
    for e in entries {
        if multi {
            println!("  {:<width$}  {:<rwidth$}  {}", e.name, e.registry, e.description);
        } else {
            println!("  {:<width$}  {}", e.name, e.description);
        }
    }
}

fn print_info(e: &Entry) {
    println!("{}  ({})", e.name, e.registry);
    println!("  repo:          {}", e.repo);
    println!("  description:   {}", e.description);
    if !e.capabilities.is_empty() {
        println!("  capabilities:  {}", e.capabilities.join(", "));
    }
    if let Some(latest) = &e.latest {
        println!("  latest:        {latest}");
    }
    if !e.namespaces.is_empty() {
        println!("  namespaces:    {}", e.namespaces.join(", "));
    }
    if !e.topologies.is_empty() {
        println!("  topologies:    {}", e.topologies.join(", "));
    }
    if !e.shapes.is_empty() {
        println!("  shapes:        {}", e.shapes.join(", "));
    }
    if !e.requires.is_empty() {
        println!("  requires:      {}", e.requires.join(", "));
    }
}

/// Write the pinned `[plugins]` line into the manifest: replace the key if it is already
/// declared in the section, insert under an existing `[plugins]` header, or append the section.
/// Line-based on purpose — the edit touches exactly one line and preserves everything else
/// byte-for-byte (comments, ordering, formatting).
fn write_pin(manifest: &Path, name: &str, repo: &str, refv: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let pin = format!("{name} = {{ git = \"{repo}\", tag = \"{refv}\" }}");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();

    if let Some(header) = lines.iter().position(|l| l.trim() == "[plugins]") {
        let mut i = header + 1;
        while i < lines.len() && !lines[i].trim_start().starts_with('[') {
            let key = lines[i].split('=').next().unwrap_or("").trim();
            if key == name {
                lines[i] = pin;
                let out = lines.join("\n") + "\n";
                return std::fs::write(manifest, out)
                    .map_err(|e| format!("cannot write {}: {e}", manifest.display()));
            }
            i += 1;
        }
        lines.insert(header + 1, pin);
    } else {
        if !lines.last().is_none_or(|l| l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[plugins]".to_string());
        lines.push(pin);
    }
    let out = lines.join("\n") + "\n";
    std::fs::write(manifest, out).map_err(|e| format!("cannot write {}: {e}", manifest.display()))
}

/// `add <[registry:]name[@ref]>` — search-to-pinned in one motion. The registry maps the name to
/// its canonical repo and recommended pin; the MANIFEST holds the result. A fresh checkout
/// reproduces the run with zero registries configured.
fn add(spec: &str, entries: &[Entry]) -> Result<String, String> {
    // `@ref` from the right, then `registry:` from the left.
    let (rest, explicit_ref) = match spec.rsplit_once('@') {
        Some((r, v)) if !v.is_empty() => (r, Some(v.to_string())),
        _ => (spec, None),
    };
    let (registry, name) = match rest.split_once(':') {
        Some((r, n)) => (Some(r), n),
        None => (None, rest),
    };

    let candidates: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.name == name && registry.is_none_or(|r| e.registry == r))
        .collect();
    let entry = match candidates.as_slice() {
        [] => {
            return Err(match registry {
                Some(r) => format!("no plugin \"{name}\" in registry {r}"),
                None => format!(
                    "no plugin \"{name}\" in any configured registry — search first: \
                     `prova plugins {name}`"
                ),
            })
        }
        [one] => *one,
        many => {
            let regs: Vec<&str> = many.iter().map(|e| e.registry.as_str()).collect();
            return Err(format!(
                "plugin \"{name}\" exists in multiple registries: {} — disambiguate as \
                 registry:name (e.g. `prova plugins add {}:{name}`)",
                regs.join(", "),
                regs[0]
            ));
        }
    };

    let refv = explicit_ref.or_else(|| entry.latest.clone()).ok_or_else(|| {
        format!(
            "entry \"{name}\" carries no recommended pin — add an explicit ref: \
             `prova plugins add {name}@<ref>`"
        )
    })?;

    let cwd = std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    let found = home::find(&cwd)?.ok_or_else(|| {
        "no prova.toml found walking up — `add` pins into a package's manifest (run inside a \
         package, or `prova init` one)"
            .to_string()
    })?;
    write_pin(&found.manifest, name, &entry.repo, &refv)?;
    Ok(format!(
        "added to {}:\n  {name} = {{ git = \"{}\", tag = \"{refv}\" }}\nuse it now: \
         `require(\"{name}\")` in a proof",
        found.manifest.display(),
        entry.repo
    ))
}

const USAGE: &str = "usage:
  prova plugins                     list every entry across configured registries
  prova plugins <query>             search (name, description, capabilities)
  prova plugins info <name>         one entry, full detail
  prova plugins add <name>[@ref]    pin into this package's [plugins] (registry:name to disambiguate)
options: --offline (cache only) · -U/--update (force-refresh registry sources)";

/// The `prova plugins` verb. Discovery works without a manifest on purpose — like
/// `prova init --list`, an agent explores before a package exists.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut offline = false;
    let mut force = false;
    let mut words: Vec<String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--offline" => offline = true,
            "-U" | "--update" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if a.starts_with('-') => {
                eprintln!("prova: plugins: unknown flag {a}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => words.push(a),
        }
    }

    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("prova: plugins: {e}");
            return ExitCode::FAILURE;
        }
    };
    let git_opts = GitFetchOptions {
        force,
        offline,
        interval: Duration::from_secs(24 * 60 * 60),
    };
    let loaded = match load_all(&layout, &git_opts) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("prova: plugins: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Loaded { entries, warnings, errors, registry_count } = loaded;
    for w in &warnings {
        eprintln!("prova: plugins: {w}");
    }
    let multi = registry_count > 1;

    let code = match words.first().map(String::as_str) {
        None => {
            print_rows(&entries, multi);
            ExitCode::SUCCESS
        }
        Some("info") => match words.get(1) {
            None => {
                eprintln!("prova: plugins: info needs a name\n{USAGE}");
                ExitCode::from(2)
            }
            Some(name) => {
                let hits: Vec<&Entry> = entries.iter().filter(|e| &e.name == name).collect();
                if hits.is_empty() {
                    eprintln!(
                        "prova: plugins: no plugin \"{name}\" in any configured registry — \
                         search first: `prova plugins {name}`"
                    );
                    ExitCode::FAILURE
                } else {
                    for e in hits {
                        print_info(e);
                    }
                    ExitCode::SUCCESS
                }
            }
        },
        Some("add") => match words.get(1) {
            None => {
                eprintln!("prova: plugins: add needs a name\n{USAGE}");
                ExitCode::from(2)
            }
            Some(spec) => match add(spec, &entries) {
                Ok(msg) => {
                    println!("{msg}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("prova: plugins: {e}");
                    ExitCode::FAILURE
                }
            },
        },
        Some(_) => {
            let query = words.join(" ");
            let hits: Vec<Entry> = entries.iter().filter(|e| matches(e, &query)).cloned().collect();
            if hits.is_empty() {
                println!("no plugins matching \"{query}\" — `prova plugins` lists everything");
            } else {
                print_rows(&hits, multi);
            }
            ExitCode::SUCCESS
        }
    };

    // A registry that could not be served at all fails the command loud — after showing what DID
    // load, so a partial outage never silently narrows discovery.
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("prova: plugins: {e}");
        }
        return ExitCode::FAILURE;
    }
    code
}

/// The learn-slot rendering: configured registries + the search-first move. No fetch happens
/// here — learning must work offline; names and sources come from config alone.
pub fn learn_lines(cli: bool) -> String {
    let regs = XdgSystemLayout::new()
        .map_err(|e| e.to_string())
        .and_then(|l| configured(&l));
    match regs {
        Ok(regs) => {
            let width = regs.iter().map(|r| r.name.len()).max().unwrap_or(0);
            let mut out: Vec<String> = vec![
                "**Registries** (searchable plugin indexes; trust = the org you listed):".into(),
            ];
            for r in &regs {
                out.push(format!("  {:<width$}  {}", r.name, r.source));
            }
            out.push(String::new());
            out.push(if cli {
                "Before hand-writing a capability, SEARCH: `prova plugins <term>` — then \
                 `prova plugins add <name>` pins it into `[plugins]` and `require(\"<name>\")` \
                 works immediately. Add registries in `~/.config/prova/config.toml` \
                 (`[[registries]]` with `name` + `source`)."
                    .into()
            } else {
                "Before hand-writing a capability, SEARCH by shelling out: `prova plugins <term>` \
                 — then `prova plugins add <name>` pins it into `[plugins]` and \
                 `require(\"<name>\")` works immediately."
                    .into()
            });
            out.join("\n")
        }
        Err(e) => format!("Registries could not be read: {e}"),
    }
}
