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
use crate::manifest::PackageDetail;
use crate::packages::{is_git_source, GitFetchOptions};

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
    /// Free-text discovery keywords (`prova packages <query>` search). Catalog metadata — NOT the
    /// runtime capability vocabulary (`requires`/`must_run`/`prova capabilities`); the package's
    /// host-capability needs live in `requires` below. See docs/design/registry.md.
    pub keywords: Vec<String>,
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
    keywords: Vec<String>,
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
        let detail = PackageDetail {
            path: None,
            git: Some(reg.source.clone()),
            tag: None,
            branch: None,
            rev: None,
            module: None,
        };
        let (dir, lease) = crate::packages::fetch_git(&reg.source, &detail, layout, git_opts)
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

/// List `<dir>/<subdir>/*.toml`, sorted. A missing or unreadable directory is a warning naming it,
/// not a failure: a registry that serves plugins but no archetypes (or the reverse) is normal, and
/// tolerance is the contract on both scans.
fn entry_files(
    reg: &RegistryRef,
    dir: &Path,
    subdir: &str,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let entries_dir = dir.join(subdir);
    if !entries_dir.is_dir() {
        // Silent: an archetype-less plugin registry (or vice versa) is a normal shape, not a problem
        // to report on every listing.
        return Vec::new();
    }
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
    files
}

/// Read every `registry/*.toml` under a resolved registry dir. Tolerance is the contract: an
/// entry with an unknown schema major, a missing required field, or unparseable TOML is skipped
/// with a warning naming it; its siblings still serve.
fn load_entries(reg: &RegistryRef, dir: &Path, warnings: &mut Vec<String>) -> Vec<Entry> {
    let files = entry_files(reg, dir, "registry", warnings);
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
            keywords: file.keywords,
            latest: file.latest,
            namespaces: file.namespaces,
            topologies: file.topologies,
            shapes: file.shapes,
            requires: file.requires,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Archetypes — the `prova init` half of a registry
// ---------------------------------------------------------------------------------------------
//
// Archetype entries live in a SIBLING directory (`archetypes/`), not alongside plugins in
// `registry/`, for two reasons that both bite later if merged:
//
//   * **Independent namespaces.** An archetype key and a plugin name are different kinds of
//     identifier; `postgres` can reasonably be both a plugin you require and an archetype you
//     render. One directory would force them to collide.
//   * **Different projections.** A plugin entry is derived from the plugin's `[plugin]` manifest and
//     `prova plugin lint`'s shape classification. An archetype has neither — its metadata comes from
//     `archetype.yaml`, and it carries a field no plugin has (`in_package`). Squeezing both into one
//     schema would mean half the fields are always empty and the required set can't be checked.
//
// Discovery-only, exactly like the plugin half: a registry lookup happens when a human runs
// `prova init`, never during a run. Nothing about a committed `prova.toml` depends on a registry.

/// One archetype entry, as served.
#[derive(Debug, Clone)]
pub struct ArchetypeEntry {
    /// The serving registry's configured name — the disambiguator when two registries carry a key.
    pub registry: String,
    pub name: String,
    pub repo: String,
    pub description: String,
    /// The recommended ref to render, appended to `repo` as `#<latest>`. Absent means the default
    /// branch, which is unpinned — reported when it happens.
    pub latest: Option<String>,
    /// Whether this archetype may render inside an already-initialized package. Publisher policy,
    /// not the consumer's: only the archetype knows whether it creates a package or augments one.
    pub in_package: Option<String>,
}

impl ArchetypeEntry {
    /// The archetype source to render: the repo, pinned to `latest` when the entry recommends one.
    ///
    /// The `#ref` suffix is appended only for a **git** source. A registry may serve a local path (the
    /// same source classification plugins use — handy for an org-internal registry pointing into a
    /// monorepo), and `path#ref` is not a path: archetect would look for a directory with `#ref` in its
    /// name and report it missing. A path has no ref semantics, so `latest` is dropped rather than
    /// concatenated — [`latest_ignored`] is how the caller learns it happened.
    ///
    /// An http(s) repo is normalized to carry the `.git` suffix archetect's source detection
    /// requires: registries serve browser-shaped URLs (`https://host/org/repo` — that is what
    /// registration derives from the repo's own identity), while archetect classifies an https URL
    /// as a git source only when its path contains `.git`. Without the bridge, every
    /// registry-resolved render failed with Source-not-found — found live the day the first
    /// registry-only archetype (`rust-project`) shipped; the built-in catalog never hit it because
    /// its pinned sources spell `.git` by hand.
    pub fn source(&self) -> String {
        let repo = if (self.repo.starts_with("https://") || self.repo.starts_with("http://"))
            && !self.repo.trim_end_matches('/').ends_with(".git")
        {
            format!("{}.git", self.repo.trim_end_matches('/'))
        } else {
            self.repo.clone()
        };
        match &self.latest {
            Some(r) if is_git_source(&self.repo) => format!("{repo}#{r}"),
            _ => repo,
        }
    }

    /// Whether this entry recommends a pin that [`source`](Self::source) cannot apply — a `latest` on a
    /// local-path repo. Silently dropping a pin is the kind of thing that surprises later, so it is
    /// reported rather than swallowed.
    pub fn latest_ignored(&self) -> bool {
        self.latest.is_some() && !is_git_source(&self.repo)
    }
}

/// The on-disk archetype entry, parsed as leniently as the plugin one: every field optional,
/// unknown keys ignored, requiredness checked after parse so a miss is a per-entry warning.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ArchetypeFile {
    schema: Option<i64>,
    name: Option<String>,
    repo: Option<String>,
    description: Option<String>,
    latest: Option<String>,
    in_package: Option<String>,
}

/// Read every `archetypes/*.toml` under a resolved registry dir.
fn load_archetypes(reg: &RegistryRef, dir: &Path, warnings: &mut Vec<String>) -> Vec<ArchetypeEntry> {
    let mut out = Vec::new();
    for path in entry_files(reg, dir, "archetypes", warnings) {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<entry>")
            .to_string();
        let mut skip = |why: String| {
            warnings.push(format!(
                "registry {}: skipping archetype {stem}: {why}",
                reg.name
            ));
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                skip(format!("cannot read: {e}"));
                continue;
            }
        };
        let file: ArchetypeFile = match toml::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                skip(format!("invalid TOML: {e}"));
                continue;
            }
        };
        let schema = file.schema.unwrap_or(KNOWN_SCHEMA);
        if schema != KNOWN_SCHEMA {
            skip(format!(
                "schema {schema} is newer than this binary understands ({KNOWN_SCHEMA})"
            ));
            continue;
        }
        let (Some(name), Some(repo), Some(description)) = (file.name, file.repo, file.description)
        else {
            skip("missing a required field (name, repo, description)".to_string());
            continue;
        };
        out.push(ArchetypeEntry {
            registry: reg.name.clone(),
            name,
            repo,
            description,
            latest: file.latest,
            in_package: file.in_package,
        });
    }
    out
}

/// Every archetype the configured registries serve, sorted by (name, registry). Warnings are
/// returned rather than printed so the caller decides whether this is a listing or a lookup.
pub fn archetypes(
    layout: &dyn SystemLayout,
    warnings: &mut Vec<String>,
) -> Result<Vec<ArchetypeEntry>, String> {
    let git_opts = GitFetchOptions::default();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for reg in &configured(layout)? {
        match registry_dir(reg, layout, &git_opts) {
            Ok((dir, _lease)) => out.extend(load_archetypes(reg, &dir, warnings)),
            Err(e) => errors.push(e),
        }
    }
    // A registry that cannot be served at all is worth saying out loud, but it must not sink a
    // lookup that the remaining registries can answer — the same degrade-don't-break rule the
    // per-entry tolerance follows.
    warnings.extend(errors);
    out.sort_by(|a, b| (&a.name, &a.registry).cmp(&(&b.name, &b.registry)));
    Ok(out)
}

/// Look one archetype key up across the configured registries. `Ok(None)` means no registry serves
/// it. When two registries do, the first in configured order wins and the shadowing is reported —
/// silently preferring one would make `prova init <key>` depend on config order nobody can see.
pub fn lookup_archetype(
    key: &str,
    layout: &dyn SystemLayout,
    warnings: &mut Vec<String>,
) -> Result<Option<ArchetypeEntry>, String> {
    let all = archetypes(layout, warnings)?;
    let hits: Vec<ArchetypeEntry> = all.into_iter().filter(|a| a.name == key).collect();
    if hits.len() > 1 {
        let names = hits
            .iter()
            .map(|a| a.registry.clone())
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "archetype {key:?} is served by more than one registry ({names}) — using {}",
            hits[0].registry
        ));
    }
    Ok(hits.into_iter().next())
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

/// Whether an entry matches a search term: substring over name, description, and keywords,
/// case-insensitive. A few hundred entries in memory — no index, no query language.
fn matches(e: &Entry, q: &str) -> bool {
    let q = q.to_lowercase();
    e.name.to_lowercase().contains(&q)
        || e.description.to_lowercase().contains(&q)
        || e.keywords.iter().any(|c| c.to_lowercase().contains(&q))
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
    if !e.keywords.is_empty() {
        println!("  keywords:      {}", e.keywords.join(", "));
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
                Some(r) => format!("no package \"{name}\" in registry {r}"),
                None => format!(
                    "no package \"{name}\" in any configured registry — search first: \
                     `prova plugins {name}`"
                ),
            })
        }
        [one] => *one,
        many => {
            let regs: Vec<&str> = many.iter().map(|e| e.registry.as_str()).collect();
            return Err(format!(
                "package \"{name}\" exists in multiple registries: {} — disambiguate as \
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
  prova packages                    list every entry across configured registries
  prova packages <query>            search (name, description, keywords)
  prova packages info <name>        one entry, full detail
  prova packages add <name>[@ref]   pin into this package's [dependencies] (registry:name to disambiguate)
options: --offline (cache only) · -U/--update (force-refresh registry sources)";

/// The `prova packages` verb. Discovery works without a manifest on purpose — like
/// `prova init --list`, an agent explores before a package exists.
/// Search the configured registries for the MCP `packages` tool: load every entry (cached, default
/// freshness — no forced fetch), optionally filter by `query` with the SAME substring match `run`
/// uses (name + description + keywords), and return the hits plus any per-entry warnings. Sharing
/// `load_all` + `matches` is what keeps the MCP `packages` tool and the CLI `prova packages` verb
/// searching identically — one registry, one result, two front-ends.
pub(crate) fn search_entries(
    layout: &dyn SystemLayout,
    query: Option<&str>,
) -> Result<(Vec<Entry>, Vec<String>), String> {
    let loaded = load_all(layout, &GitFetchOptions::default())?;
    let entries = match query {
        Some(q) if !q.trim().is_empty() => {
            let q = q.trim();
            loaded.entries.into_iter().filter(|e| matches(e, q)).collect()
        }
        _ => loaded.entries,
    };
    Ok((entries, loaded.warnings))
}

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
                eprintln!("prova: packages: unknown flag {a}\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => words.push(a),
        }
    }

    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("prova: packages: {e}");
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
            eprintln!("prova: packages: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Loaded { entries, warnings, errors, registry_count } = loaded;
    for w in &warnings {
        eprintln!("prova: packages: {w}");
    }
    let multi = registry_count > 1;

    let code = match words.first().map(String::as_str) {
        None => {
            print_rows(&entries, multi);
            ExitCode::SUCCESS
        }
        Some("info") => match words.get(1) {
            None => {
                eprintln!("prova: packages: info needs a name\n{USAGE}");
                ExitCode::from(2)
            }
            Some(name) => {
                let hits: Vec<&Entry> = entries.iter().filter(|e| &e.name == name).collect();
                if hits.is_empty() {
                    eprintln!(
                        "prova: packages: no package \"{name}\" in any configured registry — \
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
                eprintln!("prova: packages: add needs a name\n{USAGE}");
                ExitCode::from(2)
            }
            Some(spec) => match add(spec, &entries) {
                Ok(msg) => {
                    println!("{msg}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("prova: packages: {e}");
                    ExitCode::FAILURE
                }
            },
        },
        Some(_) => {
            let query = words.join(" ");
            let hits: Vec<Entry> = entries.iter().filter(|e| matches(e, &query)).cloned().collect();
            if hits.is_empty() {
                println!("no packages matching \"{query}\" — `prova packages` lists everything");
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
            eprintln!("prova: packages: {e}");
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
                 `prova packages add <name>` pins it into `[dependencies]` and `require(\"<name>\")` \
                 works immediately. Add registries in `~/.config/prova/config.toml` \
                 (`[[registries]]` with `name` + `source`)."
                    .into()
            } else {
                "Before hand-writing a capability, SEARCH by shelling out: `prova plugins <term>` \
                 — then `prova packages add <name>` pins it into `[dependencies]` and \
                 `require(\"<name>\")` works immediately."
                    .into()
            });
            out.join("\n")
        }
        Err(e) => format!("Registries could not be read: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::ArchetypeEntry;

    fn entry(repo: &str, latest: Option<&str>) -> ArchetypeEntry {
        ArchetypeEntry {
            registry: "test".into(),
            name: "probe".into(),
            repo: repo.into(),
            description: String::new(),
            latest: latest.map(String::from),
            in_package: None,
        }
    }

    // Archetect's https detection requires `.git` in the path; registries serve browser-shaped
    // URLs. The composed render source must bridge, or every registry-resolved render fails with
    // Source-not-found (as `rust-project` did, live, the day it registered).
    #[test]
    fn https_repo_gains_the_git_suffix_archetect_requires() {
        assert_eq!(
            entry("https://github.com/acme/acme-arch", Some("v1.0")).source(),
            "https://github.com/acme/acme-arch.git#v1.0"
        );
        assert_eq!(
            entry("https://github.com/acme/acme-arch", None).source(),
            "https://github.com/acme/acme-arch.git"
        );
        // Already-suffixed (and trailing-slash) forms are left alone rather than double-suffixed.
        assert_eq!(
            entry("https://github.com/acme/acme-arch.git", Some("v2")).source(),
            "https://github.com/acme/acme-arch.git#v2"
        );
        assert_eq!(
            entry("https://github.com/acme/acme-arch/", None).source(),
            "https://github.com/acme/acme-arch.git"
        );
    }

    #[test]
    fn local_paths_stay_verbatim_and_drop_the_pin() {
        // A path has no ref semantics and no `.git` requirement — verbatim, pin dropped,
        // `latest_ignored` is how the caller learns it happened.
        let e = entry("/registry/acme-arch", Some("v7"));
        assert_eq!(e.source(), "/registry/acme-arch");
        assert!(e.latest_ignored());
    }

    #[test]
    fn ssh_sources_are_not_rewritten() {
        assert_eq!(
            entry("git@github.com:acme/acme-arch.git", Some("v1")).source(),
            "git@github.com:acme/acme-arch.git#v1"
        );
    }
}
