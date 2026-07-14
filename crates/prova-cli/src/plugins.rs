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
/// `base_dir` is the manifest's directory (local paths resolve relative to it). Returns `name → file`
/// or a human-readable error naming the plugin that failed.
pub fn resolve_plugins(
    plugins: &BTreeMap<String, PluginSource>,
    base_dir: &Path,
    layout: &dyn SystemLayout,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut resolved = BTreeMap::new();
    for (name, source) in plugins {
        let detail = match source {
            PluginSource::Path(p) => PluginDetail {
                path: Some(p.clone()),
                ..Default::default()
            },
            PluginSource::Detailed(d) => d.clone(),
        };
        let file = resolve_one(name, &detail, base_dir, layout)
            .map_err(|e| format!("plugin {name:?}: {e}"))?;
        resolved.insert(name.clone(), file);
    }
    Ok(resolved)
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

        let resolved = resolve_plugins(&plugins, &dir, &layout).expect("resolve");
        assert_eq!(resolved["greet"], file);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_local_plugin_is_an_error() {
        let dir = std::env::temp_dir().join("prova-plugin-missing-test");
        let mut plugins = BTreeMap::new();
        plugins.insert("nope".to_string(), PluginSource::Path("nope.lua".into()));
        let layout = RootedSystemLayout::new(&dir);
        let err = resolve_plugins(&plugins, &dir, &layout).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }
}
