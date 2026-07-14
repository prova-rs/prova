//! Resolving `[plugins]` from `prova.toml` into concrete files the engine's searcher can load.
//!
//! Local sources resolve straight to a path. Git sources are fetched — shelling to `git`, like
//! archetect fetches archetype sources — into the layout's plugin cache, pinned by ref, so a repeat
//! run reuses the checkout instead of re-cloning. The result is a `name → file` map handed to
//! `RunConfig::with_named_plugin`, making the manifest the authoritative, pinned plugin source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use prova_core::SystemLayout;

use crate::manifest::{PluginDetail, PluginSource};

/// Resolve every declared plugin to a concrete `.lua` file, fetching git sources into the cache.
/// `base_dir` is the manifest's directory (local paths resolve relative to it). `sources` are the
/// registered `[sources]` aliases used to expand shorthands. Returns `name → file` or a
/// human-readable error naming the plugin that failed.
pub fn resolve_plugins(
    plugins: &BTreeMap<String, PluginSource>,
    base_dir: &Path,
    layout: &dyn SystemLayout,
    sources: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut resolved = BTreeMap::new();
    for (name, source) in plugins {
        let detail = match source {
            // A bare string is classified: a git URL or org/repo shorthand becomes a git source; a
            // path stays a path. (The table form is already explicit.)
            PluginSource::Path(s) => classify_string_source(s, sources)
                .map_err(|e| format!("plugin {name:?}: {e}"))?,
            PluginSource::Detailed(d) => d.clone(),
        };
        let file = resolve_one(name, &detail, base_dir, layout)
            .map_err(|e| format!("plugin {name:?}: {e}"))?;
        resolved.insert(name.clone(), file);
    }
    Ok(resolved)
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
            return Ok(git_detail(format!("https://github.com/{core}"), Some(reference)));
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
) -> Result<PathBuf, String> {
    match (&detail.path, &detail.git) {
        (Some(_), Some(_)) => Err("set either `path` or `git`, not both".into()),
        (Some(path), None) => {
            let root = resolve_relative(base_dir, path);
            module_file(&root, name, detail.module.as_deref())
        }
        (None, Some(git)) => {
            let checkout = fetch_git(git, detail, layout)?;
            module_file(&checkout, name, detail.module.as_deref())
        }
        (None, None) => Err("needs a `path` or a `git` source".into()),
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

/// Pick the module file inside a resolved directory (or accept a direct file): an explicit `module`,
/// else `<name>.lua`, else `init.lua`.
fn module_file(root: &Path, name: &str, module: Option<&str>) -> Result<PathBuf, String> {
    if root.is_file() {
        return Ok(root.to_path_buf());
    }
    if !root.exists() {
        return Err(format!("{} does not exist", root.display()));
    }
    if let Some(m) = module {
        let candidate = root.join(m);
        return if candidate.is_file() {
            Ok(candidate)
        } else {
            Err(format!("module {m:?} not found at {}", candidate.display()))
        };
    }
    for candidate in [root.join(format!("{name}.lua")), root.join("init.lua")] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "no `{name}.lua` or `init.lua` in {} (set `module` to point at the file)",
        root.display()
    ))
}

/// Fetch a git plugin into the layout's plugin cache, pinned by ref, and return the checkout dir. A
/// checkout that already exists is reused (tag/rev pins are immutable; a branch is cached on first
/// fetch — prefer `tag`/`rev` for reproducibility).
fn fetch_git(url: &str, detail: &PluginDetail, layout: &dyn SystemLayout) -> Result<PathBuf, String> {
    let (pin, label): (Option<&str>, String) = match (&detail.tag, &detail.branch, &detail.rev) {
        (Some(t), _, _) => (Some(t), format!("tag-{t}")),
        (_, Some(b), _) => (Some(b), format!("branch-{b}")),
        (_, _, Some(r)) => (Some(r), format!("rev-{r}")),
        _ => (None, "default".to_string()),
    };
    let dest = layout
        .plugin_cache_dir()
        .join(sanitize(url))
        .join(sanitize(&label));

    // Reuse an existing non-empty checkout.
    if dest.join(".git").is_dir() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create cache dir {}: {e}", parent.display()))?;
    }

    // tag/branch clone shallowly with `--branch`; a rev needs history, so clone then checkout.
    let dest_str = dest.to_string_lossy().to_string();
    match (&detail.rev, pin) {
        (Some(rev), _) => {
            run_git(&["clone", url, &dest_str], None)?;
            run_git(&["checkout", rev], Some(&dest))?;
        }
        (None, Some(reference)) => {
            run_git(&["clone", "--depth", "1", "--branch", reference, url, &dest_str], None)?;
        }
        (None, None) => {
            run_git(&["clone", "--depth", "1", url, &dest_str], None)?;
        }
    }
    Ok(dest)
}

/// Run `git` with args (optionally in `cwd`), returning a readable error on failure.
fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run git (is it installed?): {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(())
}

/// Make a filesystem-safe directory component from a URL or ref (keep it recognizable).
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
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

        let resolved = resolve_plugins(&plugins, &dir, &layout, &BTreeMap::new()).expect("resolve");
        assert_eq!(resolved["greet"], file);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_local_plugin_is_an_error() {
        let dir = std::env::temp_dir().join("prova-plugin-missing-test");
        let mut plugins = BTreeMap::new();
        plugins.insert("nope".to_string(), PluginSource::Path("nope.lua".into()));
        let layout = RootedSystemLayout::new(&dir);
        let err = resolve_plugins(&plugins, &dir, &layout, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("nope"), "{err}");
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
        assert_eq!(git(&d), ("https://github.com/acme/prova-redis", Some("v1.2")));

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
        sources.insert("mirror".to_string(), "https://git.acme.io/plugins".to_string());

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
        for p in ["./plugins/greet.lua", "greet.lua", "../shared/x.lua", "/abs/x.lua"] {
            let d = classify_string_source(p, &none).unwrap();
            assert_eq!(d.path.as_deref(), Some(p), "{p} should be a path");
            assert!(d.git.is_none(), "{p} should not be git");
        }
    }
}
