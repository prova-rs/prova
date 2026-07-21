//! Resolving `[plugins]` from `prova.toml` into concrete files the engine's searcher can load.
//!
//! Local sources resolve straight to a path. Git sources are fetched via the shared
//! `archetect-git-cache` crate — the same TTL + remote-hash freshness gate archetect uses — into the
//! layout's plugin cache, pinned by ref, so a repeat run reuses the checkout (and, once the interval
//! lapses, cheaply confirms the remote hasn't moved before pulling). A directory plugin may carry a
//! `prova-plugin.toml`
//! (the analogue of `archetype.yaml`): it declares the `entry` file — so resolution no longer depends
//! on the consumer's alias matching a filename — plus a compatibility range (`requires.prova`) and
//! metadata. The result is handed to `RunConfig`, making the manifest the authoritative plugin source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use camino::Utf8PathBuf;
use prova_core::SystemLayout;
use serde::Deserialize;

use crate::manifest::{PluginDetail, PluginSource};

/// How git plugin sources should be refreshed on this run — the caller (the run flow) derives these
/// from `[updates]` in the manifest and the `-U`/`--offline` flags, and threads them to `fetch_git`.
#[derive(Debug, Clone)]
pub struct GitFetchOptions {
    /// Skip the freshness gates and always fetch (`-U`/`--update`, or `[updates] force`).
    pub force: bool,
    /// Never touch the network; error if a required ref isn't already cached (`--offline`).
    pub offline: bool,
    /// TTL window — within it a cached checkout is reused with no network at all.
    pub interval: Duration,
}

impl Default for GitFetchOptions {
    fn default() -> Self {
        GitFetchOptions {
            force: false,
            offline: false,
            interval: crate::manifest::UpdatesSection::DEFAULT_INTERVAL,
        }
    }
}

/// The fully-resolved plugin set to hand to the engine: `named` maps each consumer require-name to
/// its entry file; `namespaces` maps each plugin's canonical name to its root dir (for intra-plugin
/// `require`s).
#[derive(Debug, Default, Clone)]
pub struct ResolvedPlugins {
    pub named: BTreeMap<String, PathBuf>,
    pub namespaces: BTreeMap<String, PathBuf>,
    /// Each plugin's root directory (the checkout dir, or a local dir/file's parent) keyed by
    /// canonical name — where its `prova-plugin.toml` and `library/` annotation stub live. Used to
    /// sync IDE annotations into the project's `annotations/` dir.
    pub roots: BTreeMap<String, PathBuf>,
    /// The directory scanned for undeclared (ambient) plugins — the manifest's `[run] plugin_root`,
    /// absolutised against the project root. `None` unless declared: there is no built-in root, so
    /// resolution is always readable off `prova.toml`.
    pub search_root: Option<PathBuf>,
}

/// A plugin's own manifest (`prova-plugin.toml`) — the analogue of `archetype.yaml`. All fields are
/// optional; a plugin without a manifest just falls back to filename conventions and declares no
/// compatibility constraint.
#[derive(Debug, Deserialize, Default)]
struct PluginManifest {
    #[serde(default)]
    plugin: PluginMeta,
    #[serde(default)]
    requires: PluginRequires,
}

#[derive(Debug, Deserialize, Default)]
struct PluginMeta {
    /// Canonical name — the namespace for intra-plugin `require`s. Defaults to the consumer's key.
    name: Option<String>,
    /// The entry file, relative to the plugin root (e.g. `rabbitmq.lua` or `src/rabbitmq.lua`).
    entry: Option<String>,
    #[allow(dead_code)]
    description: Option<String>,
    #[allow(dead_code)]
    license: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct PluginRequires {
    /// The range of prova versions this plugin is compatible with (semver `VersionReq`, e.g. `^0.1`).
    prova: Option<String>,
}

/// Resolve every declared plugin to a concrete entry file (fetching git sources into the cache) and
/// register its namespace. `base_dir` is the manifest's directory (local paths resolve relative to
/// it); `sources` are the `[sources]` aliases; `prova_version` is the running version, checked
/// against each plugin's `requires.prova`. Errors name the plugin that failed.
pub fn resolve_plugins(
    plugins: &BTreeMap<String, PluginSource>,
    base_dir: &Path,
    layout: &dyn SystemLayout,
    sources: &BTreeMap<String, String>,
    prova_version: &str,
    git_opts: &GitFetchOptions,
) -> Result<ResolvedPlugins, String> {
    let mut resolved = ResolvedPlugins::default();
    for (name, source) in plugins {
        let detail = match source {
            // A bare string is classified: a git URL or org/repo shorthand becomes a git source; a
            // path stays a path. (The table form is already explicit.)
            PluginSource::Path(s) => {
                classify_string_source(s, sources).map_err(|e| format!("plugin {name:?}: {e}"))?
            }
            PluginSource::Detailed(d) => d.clone(),
        };
        let one = resolve_one(name, &detail, base_dir, layout, prova_version, git_opts)
            .map_err(|e| format!("plugin {name:?}: {e}"))?;
        // The entry's directory is the module root — where sibling `require`s resolve.
        if let Some(dir) = one.entry.parent() {
            resolved
                .namespaces
                .insert(one.canonical.clone(), dir.to_path_buf());
        }
        resolved.roots.insert(one.canonical.clone(), one.root);
        resolved.named.insert(name.clone(), one.entry);
    }
    Ok(resolved)
}

/// One resolved plugin: its entry file, canonical namespace name, and root directory (where the
/// `prova-plugin.toml` and `library/` annotation stub live).
struct ResolvedOne {
    entry: PathBuf,
    canonical: String,
    root: PathBuf,
}

/// Classify a bare string plugin source into a `PluginDetail`:
/// - a git URL (`https://…`, `git@…`, `….git`) → a `git` source (with an optional trailing `@ref`);
/// - `host:org/repo[@ref]` where `host` is `github`/`gh`/`gitlab`/`gl` or a registered `[sources]`
///   alias → the expanded `git` source;
/// - a bare `org/repo@ref` (an `@ref` is required, so a plain relative path is never mistaken for a
///   remote) → `https://github.com/org/repo` at that ref;
/// - anything else → a local `path`.
///
/// The `@ref` becomes the `tag` field, which `git clone --branch` accepts for **either** a tag or a
/// branch; pin to a commit with the explicit table form (`{ git = "…", rev = "…" }`).
fn classify_string_source(
    s: &str,
    sources: &BTreeMap<String, String>,
) -> Result<PluginDetail, String> {
    if is_git_url(s) {
        let (url, reference) = split_ref(s);
        return Ok(git_detail(url.to_string(), reference));
    }
    if let Some((prefix, rest)) = s.split_once(':') {
        if let Some(base) = expand_prefix(prefix, sources) {
            let (path, reference) = split_ref(rest);
            return Ok(git_detail(join_url(&base, path), reference));
        }
    }
    if let (core, Some(reference)) = split_ref(s) {
        if is_org_repo(core) {
            return Ok(git_detail(
                format!("https://github.com/{core}"),
                Some(reference),
            ));
        }
    }
    Ok(PluginDetail {
        path: Some(s.to_string()),
        ..Default::default()
    })
}

fn git_detail(url: String, reference: Option<String>) -> PluginDetail {
    PluginDetail {
        git: Some(url),
        tag: reference,
        ..Default::default()
    }
}

/// A recognizable git URL (not a shorthand or a path).
fn is_git_url(s: &str) -> bool {
    s.contains("://") || s.starts_with("git@") || s.ends_with(".git")
}

/// Split a trailing `@ref` off, but only when the `@` is after the last `/`, so `git@host:…` and
/// `user@host` URLs are left intact.
fn split_ref(s: &str) -> (&str, Option<String>) {
    if let Some(at) = s.rfind('@') {
        let after_last_slash = s.rfind('/').is_none_or(|slash| at > slash);
        if after_last_slash {
            return (&s[..at], Some(s[at + 1..].to_string()));
        }
    }
    (s, None)
}

/// Expand a shorthand prefix to a base URL: a registered `[sources]` alias (itself a `host:org`
/// shorthand or a base URL), or a built-in host (`github`/`gh`, `gitlab`/`gl`). `None` if unknown
/// (so `C:\path` and the like fall through to a local path).
fn expand_prefix(prefix: &str, sources: &BTreeMap<String, String>) -> Option<String> {
    if let Some(base) = sources.get(prefix) {
        return Some(expand_base(base));
    }
    known_host(prefix).map(str::to_string)
}

fn known_host(prefix: &str) -> Option<&'static str> {
    match prefix {
        "github" | "gh" => Some("https://github.com"),
        "gitlab" | "gl" => Some("https://gitlab.com"),
        _ => None,
    }
}

/// Expand a `[sources]` value — a full base URL or a `host:org` shorthand — into a base URL.
fn expand_base(base: &str) -> String {
    if base.contains("://") {
        return base.trim_end_matches('/').to_string();
    }
    if let Some((host, org)) = base.split_once(':') {
        if let Some(h) = known_host(host) {
            return format!("{h}/{org}");
        }
    }
    base.to_string()
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

/// Does `s` look like `org/repo` (one slash, repo-name characters, not a path)?
fn is_org_repo(s: &str) -> bool {
    if s.starts_with('.') || s.starts_with('/') || s.starts_with('~') || s.contains('\\') {
        return false;
    }
    let parts: Vec<&str> = s.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

fn resolve_one(
    name: &str,
    detail: &PluginDetail,
    base_dir: &Path,
    layout: &dyn SystemLayout,
    prova_version: &str,
    git_opts: &GitFetchOptions,
) -> Result<ResolvedOne, String> {
    let root = match (&detail.path, &detail.git) {
        (Some(_), Some(_)) => return Err("set either `path` or `git`, not both".into()),
        (Some(path), None) => resolve_relative(base_dir, path),
        (None, Some(git)) => fetch_git(git, detail, layout, git_opts)?,
        (None, None) => return Err("needs a `path` or a `git` source".into()),
    };

    // A directory source may carry a `prova-plugin.toml`. When the source is a direct file, its own
    // directory is the plugin root (so a single-file plugin can still declare a manifest beside it).
    let manifest_dir = if root.is_file() {
        root.parent().map(Path::to_path_buf)
    } else {
        Some(root.clone())
    };
    let manifest = manifest_dir
        .as_deref()
        .map(read_plugin_manifest)
        .transpose()?
        .flatten();

    // Compatibility gate: a plugin's `requires.prova` range must admit the running version.
    if let Some(m) = &manifest {
        if let Some(req) = &m.requires.prova {
            check_compat(req, prova_version)?;
        }
    }

    // Entry precedence: consumer `module=` override → manifest `entry` → filename conventions.
    let manifest_entry = manifest.as_ref().and_then(|m| m.plugin.entry.as_deref());
    let entry = module_file(&root, name, detail.module.as_deref(), manifest_entry)?;

    // Canonical namespace: manifest `[plugin] name`, else the consumer's key.
    let canonical = manifest
        .as_ref()
        .and_then(|m| m.plugin.name.clone())
        .unwrap_or_else(|| name.to_string());

    // Plugin root: where `prova-plugin.toml` and `library/` live (the dir for a directory source, a
    // file's parent for a single-file source).
    let plugin_root = manifest_dir.unwrap_or_else(|| root.clone());

    Ok(ResolvedOne {
        entry,
        canonical,
        root: plugin_root,
    })
}

/// The plugin namespace for a standalone entry file (`prova plugin lint <file>`): its canonical name
/// (from a sibling `prova-plugin.toml`, else the file stem) mapped to its directory, so the file's
/// own `require("<canonical>.<sub>")` siblings resolve during lint. `None` if the file has no parent.
pub fn namespace_for_file(file: &Path) -> Option<(String, PathBuf)> {
    let dir = file.parent()?;
    let canonical = read_plugin_manifest(dir)
        .ok()
        .flatten()
        .and_then(|m| m.plugin.name)
        .or_else(|| {
            file.file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })?;
    Some((canonical, dir.to_path_buf()))
}

/// Read a `prova-plugin.toml` from `dir` if present. `Ok(None)` when absent; `Err` on malformed TOML.
fn read_plugin_manifest(dir: &Path) -> Result<Option<PluginManifest>, String> {
    let path = dir.join("prova-plugin.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<PluginManifest>(&text)
            .map(Some)
            .map_err(|e| format!("invalid prova-plugin.toml: {e}")),
        Err(_) => Ok(None),
    }
}

/// Check the running prova version against a plugin's `requires.prova` semver range. On 0.x, the
/// minor is the breaking axis, which `semver`'s `VersionReq` handles (`^0.1` = `>=0.1.0, <0.2.0`).
fn check_compat(req: &str, prova_version: &str) -> Result<(), String> {
    let range = semver::VersionReq::parse(req)
        .map_err(|e| format!("invalid `requires.prova` range {req:?}: {e}"))?;
    let version = semver::Version::parse(prova_version)
        .map_err(|e| format!("cannot parse prova version {prova_version:?}: {e}"))?;
    if range.matches(&version) {
        Ok(())
    } else {
        Err(format!(
            "requires prova {req} but this is {prova_version} \
             (upgrade prova, or pin an older plugin version)"
        ))
    }
}

/// Resolve `path` against `base_dir` unless it is already absolute.
fn resolve_relative(base_dir: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        base_dir.join(p)
    }
}

/// Pick the entry file inside a resolved directory (or accept a direct file). Precedence:
/// the consumer's `module=` override → the manifest's `entry` → `init.lua` → `<name>.lua`. The
/// manifest/`init.lua` are the robust paths; `<name>.lua` is a last-ditch back-compat fallback that
/// couples the file to the consumer's alias (which is exactly why a published plugin declares
/// `entry` in `prova-plugin.toml` instead).
fn module_file(
    root: &Path,
    name: &str,
    module: Option<&str>,
    manifest_entry: Option<&str>,
) -> Result<PathBuf, String> {
    if root.is_file() {
        return Ok(root.to_path_buf());
    }
    if !root.exists() {
        return Err(format!("{} does not exist", root.display()));
    }
    // A declared entry (consumer override, then the plugin's own manifest) must exist if given.
    for (source, declared) in [
        ("module", module),
        ("prova-plugin.toml entry", manifest_entry),
    ] {
        if let Some(rel) = declared {
            let candidate = root.join(rel);
            return if candidate.is_file() {
                Ok(candidate)
            } else {
                Err(format!(
                    "{source} {rel:?} not found at {}",
                    candidate.display()
                ))
            };
        }
    }
    // Zero-config conventions: idiomatic `init.lua` first, then the (frail) alias-named file.
    for candidate in [root.join("init.lua"), root.join(format!("{name}.lua"))] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "no entry file in {} (add `prova-plugin.toml` with `entry = \"…\"`, an `init.lua`, \
         or set `module`)",
        root.display()
    ))
}

/// Fetch a git plugin into the layout's plugin cache (one dir per `(url, ref)`, so two plugins can
/// pin the same repo at different refs), pinned by ref, and return the checkout dir. Freshness is the
/// shared crate's two-gate check: a `tag`/`rev` pin is immutable and never re-probed; a `branch` (or
/// the default branch) is TTL-gated and, once stale, confirmed against the remote hash before any
/// pull. Prefer `tag`/`rev` for reproducibility.
fn fetch_git(
    url: &str,
    detail: &PluginDetail,
    layout: &dyn SystemLayout,
    git_opts: &GitFetchOptions,
) -> Result<PathBuf, String> {
    use archetect_git_cache::{FetchOptions, Freshness, RefPin};

    // Map the pin to (immutability, ref, cache-label). A tag/rev never moves; a branch does.
    let (pin, gitref, label): (RefPin, Option<&str>, String) =
        match (&detail.tag, &detail.branch, &detail.rev) {
            (Some(t), _, _) => (RefPin::Immutable, Some(t.as_str()), format!("tag-{t}")),
            (_, Some(b), _) => (RefPin::Mutable, Some(b.as_str()), format!("branch-{b}")),
            (_, _, Some(r)) => (RefPin::Immutable, Some(r.as_str()), format!("rev-{r}")),
            _ => (RefPin::Mutable, None, "default".to_string()),
        };
    let dest = layout
        .plugin_cache_dir()
        .join(sanitize(url))
        .join(sanitize(&label));
    let cache_path = Utf8PathBuf::from_path_buf(dest.clone())
        .map_err(|_| format!("plugin cache path is not UTF-8: {}", dest.display()))?;

    let opts = FetchOptions {
        force: git_opts.force,
        offline: git_opts.offline,
        interval: git_opts.interval,
        pin,
    };
    let outcome = archetect_git_cache::fetch(url, gitref, &cache_path, &opts)
        .map_err(|e| format!("fetching {url}: {e}"))?;

    // Only speak up when the cache actually changed — a silent freshness confirmation stays silent.
    match outcome.freshness {
        Freshness::Cloned => eprintln!("prova: fetching plugin {url}"),
        Freshness::Updated => eprintln!("prova: updating plugin {url}"),
        Freshness::UpToDate { .. } => {}
    }
    Ok(dest)
}

/// Make a filesystem-safe directory component from a URL or ref (keep it recognizable).
pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prova_core::RootedSystemLayout;

    #[test]
    fn sanitize_makes_safe_components() {
        assert_eq!(
            sanitize("https://github.com/acme/prova-nats.git"),
            "https___github.com_acme_prova-nats.git"
        );
        assert_eq!(sanitize("v1.0.0"), "v1.0.0");
    }

    #[test]
    fn resolves_a_local_file_path() {
        let dir = std::env::temp_dir().join("prova-plugin-local-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("greet.lua");
        std::fs::write(&file, "return {}").unwrap();

        let mut plugins = BTreeMap::new();
        plugins.insert("greet".to_string(), PluginSource::Path("greet.lua".into()));
        let layout = RootedSystemLayout::new(dir.join("home"));

        let resolved = resolve_plugins(
            &plugins,
            &dir,
            &layout,
            &BTreeMap::new(),
            "0.1.1",
            &GitFetchOptions::default(),
        )
        .expect("resolve");
        assert_eq!(resolved.named["greet"], file);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_local_plugin_is_an_error() {
        let dir = std::env::temp_dir().join("prova-plugin-missing-test");
        let mut plugins = BTreeMap::new();
        plugins.insert("nope".to_string(), PluginSource::Path("nope.lua".into()));
        let layout = RootedSystemLayout::new(&dir);
        let err = resolve_plugins(
            &plugins,
            &dir,
            &layout,
            &BTreeMap::new(),
            "0.1.1",
            &GitFetchOptions::default(),
        )
        .unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn manifest_entry_resolves_under_a_different_alias() {
        // A repo whose entry file is `rabbitmq.lua`, declared in prova-plugin.toml — pulled under a
        // DIFFERENT consumer alias `mq`. Filename-matching would look for `mq.lua` and fail; the
        // manifest entry makes it resolve regardless of the alias, and namespaces it by canonical name.
        let dir = std::env::temp_dir().join(format!("prova-plugin-entry-{}", std::process::id()));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("rabbitmq.lua"),
            "return { container = function() end }",
        )
        .unwrap();
        std::fs::write(
            repo.join("prova-plugin.toml"),
            "[plugin]\nname = \"rabbitmq\"\nentry = \"rabbitmq.lua\"\n",
        )
        .unwrap();

        let mut plugins = BTreeMap::new();
        plugins.insert("mq".to_string(), PluginSource::Path("repo".into()));
        let layout = RootedSystemLayout::new(dir.join("home"));

        let resolved = resolve_plugins(
            &plugins,
            &dir,
            &layout,
            &BTreeMap::new(),
            "0.1.1",
            &GitFetchOptions::default(),
        )
        .expect("resolve");
        assert_eq!(resolved.named["mq"], repo.join("rabbitmq.lua"));
        // Namespaced by canonical name `rabbitmq` (from the manifest), not the alias `mq`.
        assert_eq!(resolved.namespaces["rabbitmq"], repo);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn incompatible_prova_version_is_rejected() {
        assert!(check_compat("^0.2", "0.1.1").is_err());
        assert!(check_compat(">=0.1, <0.2", "0.1.1").is_ok());
        assert!(check_compat("^0.1", "0.1.5").is_ok());
    }

    fn git(detail: &PluginDetail) -> (&str, Option<&str>) {
        (detail.git.as_deref().unwrap(), detail.tag.as_deref())
    }

    #[test]
    fn classifies_full_git_urls() {
        let none = BTreeMap::new();
        let d = classify_string_source("https://github.com/acme/prova-redis.git", &none).unwrap();
        assert_eq!(git(&d), ("https://github.com/acme/prova-redis.git", None));

        let d = classify_string_source("https://github.com/acme/prova-redis@v1.2", &none).unwrap();
        assert_eq!(
            git(&d),
            ("https://github.com/acme/prova-redis", Some("v1.2"))
        );

        // A `git@host:...` scp URL keeps its early `@`.
        let d = classify_string_source("git@github.com:acme/prova-redis.git", &none).unwrap();
        assert_eq!(git(&d), ("git@github.com:acme/prova-redis.git", None));
    }

    #[test]
    fn expands_host_prefix_shorthand() {
        let none = BTreeMap::new();
        let d = classify_string_source("github:acme/prova-redis@v1", &none).unwrap();
        assert_eq!(git(&d), ("https://github.com/acme/prova-redis", Some("v1")));

        let d = classify_string_source("gl:acme/prova-redis", &none).unwrap();
        assert_eq!(git(&d), ("https://gitlab.com/acme/prova-redis", None));
    }

    #[test]
    fn expands_registered_alias() {
        let mut sources = BTreeMap::new();
        sources.insert("acme".to_string(), "github:acme".to_string());
        sources.insert(
            "mirror".to_string(),
            "https://git.acme.io/plugins".to_string(),
        );

        let d = classify_string_source("acme:redis@v1", &sources).unwrap();
        assert_eq!(git(&d), ("https://github.com/acme/redis", Some("v1")));

        let d = classify_string_source("mirror:redis", &sources).unwrap();
        assert_eq!(git(&d), ("https://git.acme.io/plugins/redis", None));
    }

    #[test]
    fn bare_org_repo_needs_a_ref_else_is_a_path() {
        let none = BTreeMap::new();
        // With @ref → github shorthand.
        let d = classify_string_source("acme/prova-redis@v1", &none).unwrap();
        assert_eq!(git(&d), ("https://github.com/acme/prova-redis", Some("v1")));

        // Without @ref → a local path (never a surprise fetch).
        let d = classify_string_source("test-support/redis", &none).unwrap();
        assert_eq!(d.path.as_deref(), Some("test-support/redis"));
        assert!(d.git.is_none());
    }

    #[test]
    fn plain_paths_stay_paths() {
        let none = BTreeMap::new();
        for p in [
            "./plugins/greet.lua",
            "greet.lua",
            "../shared/x.lua",
            "/abs/x.lua",
        ] {
            let d = classify_string_source(p, &none).unwrap();
            assert_eq!(d.path.as_deref(), Some(p), "{p} should be a path");
            assert!(d.git.is_none(), "{p} should not be git");
        }
    }
}
