//! Locating the "prova home" — the directory that owns a project's `prova.toml`, and against which
//! every relative path in the manifest (`[run] paths`, discovered `annotations/`) is resolved.
//!
//! A project may keep its manifest in one of three places, so teams can choose how much prova lives
//! at the repo root:
//!
//! | Location | Home dir | Feel |
//! |---|---|---|
//! | `prova.toml` | the project root | flat — prova at the root, zero nesting |
//! | `prova/prova.toml` | `prova/` | visible — tests + config in one navigable dir |
//! | `.prova/prova.toml` | `.prova/` | hidden — config + generated files tucked away |
//!
//! Discovery walks **up** from the current directory (like git finding `.git`), so `prova` works
//! from anywhere inside a project. Finding **more than one** of the three candidates in the same
//! directory is a hard error: the layout is ambiguous and prova refuses to guess which is canonical.

use std::path::{Path, PathBuf};

/// A located prova home.
#[derive(Debug, Clone, PartialEq)]
pub struct Home {
    /// The project root — the ancestor directory the manifest was found under. The editor pointer
    /// (`.luarc.json`) lives here, because LuaLS binds to the workspace root the user opens, not to a
    /// nested `prova/` or `.prova/`.
    pub root: PathBuf,
    /// The home directory — where `prova.toml` lives (the root itself, or its `prova/` / `.prova/`
    /// child). All manifest-relative paths resolve against this.
    pub dir: PathBuf,
    /// The manifest file (`<dir>/prova.toml`).
    pub manifest: PathBuf,
}

/// Walk up from `start`, returning the first ancestor that holds a manifest. `Ok(None)` means no
/// manifest anywhere up to the filesystem root (an ad-hoc invocation with no project). `Err` means a
/// single directory holds more than one of the candidate manifests (ambiguous — the user must keep
/// exactly one).
pub fn find(start: &Path) -> Result<Option<Home>, String> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    for dir in start.ancestors() {
        match at(dir)? {
            Some(home) => return Ok(Some(home)),
            None => continue,
        }
    }
    Ok(None)
}

/// Build a `Home` from an explicit manifest path (`--manifest`). The home dir is the manifest's
/// parent; the project root is that same dir (an explicitly-pointed manifest defines its own root —
/// we don't second-guess where `.luarc.json` should go relative to it).
pub fn from_manifest_path(manifest: &Path) -> Home {
    let dir = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
    Home {
        root: dir.clone(),
        dir,
        manifest: manifest.to_path_buf(),
    }
}

/// A discovered manifest at some directory level: its display label, the manifest file, and the
/// (root, home) pair it implies.
struct Candidate {
    label: &'static str,
    manifest: PathBuf,
    root: PathBuf,
    home: PathBuf,
}

/// Check directory `dir` for the candidate manifests. `Ok(None)` if none present; `Ok(Some)` for
/// exactly one; `Err` if more than one (ambiguous layout).
///
/// The subtlety: `dir/prova.toml` is a *flat* manifest (home is `dir`) **unless** `dir` is itself
/// named `prova`/`.prova` — in which case it's a nested home whose real root is `dir`'s parent. That
/// name-based rule makes discovery return the same (root, home) whether invoked from the project
/// root or from inside the home dir (`cd prova && prova`).
fn at(dir: &Path) -> Result<Option<Home>, String> {
    let mut found: Vec<Candidate> = Vec::new();

    // `dir/prova.toml` — flat, or (if `dir` is a home-dir name) a nested home rooted at the parent.
    let flat = dir.join("prova.toml");
    if flat.is_file() {
        let (label, root) = match dir.file_name().and_then(|s| s.to_str()) {
            Some("prova") => (
                "prova/prova.toml",
                dir.parent().unwrap_or(dir).to_path_buf(),
            ),
            Some(".prova") => (
                ".prova/prova.toml",
                dir.parent().unwrap_or(dir).to_path_buf(),
            ),
            _ => ("prova.toml", dir.to_path_buf()),
        };
        found.push(Candidate {
            label,
            manifest: flat,
            root,
            home: dir.to_path_buf(),
        });
    }
    // `dir/prova/prova.toml` — a visible nested home rooted at `dir`.
    let vis = dir.join("prova").join("prova.toml");
    if vis.is_file() {
        found.push(Candidate {
            label: "prova/prova.toml",
            manifest: vis,
            root: dir.to_path_buf(),
            home: dir.join("prova"),
        });
    }
    // `dir/.prova/prova.toml` — a hidden nested home rooted at `dir`.
    let hid = dir.join(".prova").join("prova.toml");
    if hid.is_file() {
        found.push(Candidate {
            label: ".prova/prova.toml",
            manifest: hid,
            root: dir.to_path_buf(),
            home: dir.join(".prova"),
        });
    }

    match found.as_slice() {
        [] => Ok(None),
        [c] => Ok(Some(Home {
            root: c.root.clone(),
            dir: c.home.clone(),
            manifest: c.manifest.clone(),
        })),
        many => {
            let names: Vec<&str> = many.iter().map(|c| c.label).collect();
            Err(format!(
                "ambiguous prova manifest in {}: found {} — keep exactly one",
                dir.display(),
                names.join(" and ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory tree, removed on drop.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let dir = std::env::temp_dir().join(format!("prova-home-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
        fn write(&self, rel: &str, body: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn flat_manifest_home_is_the_root() {
        let t = Tmp::new("flat");
        t.write("prova.toml", "[run]\npaths=[\".\"]\n");
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.root, home.dir);
        assert_eq!(
            home.manifest,
            t.0.canonicalize().unwrap().join("prova.toml")
        );
    }

    #[test]
    fn hidden_manifest_home_is_dot_prova() {
        let t = Tmp::new("hidden");
        t.write(".prova/prova.toml", "[run]\npaths=[\"suites\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.root, root);
        assert_eq!(home.dir, root.join(".prova"));
    }

    #[test]
    fn visible_manifest_home_is_prova_dir() {
        let t = Tmp::new("visible");
        t.write("prova/prova.toml", "[run]\npaths=[\"suites\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root.join("prova"));
    }

    #[test]
    fn discovery_from_inside_visible_home_still_roots_at_parent() {
        // `cd prova && prova` must resolve the SAME (root, home) as running from the project root —
        // otherwise `.luarc.json` lands inside prova/ instead of at the real root.
        let t = Tmp::new("inside-visible");
        t.write("prova/prova.toml", "[run]\npaths=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&root.join("prova")).unwrap().unwrap();
        assert_eq!(home.root, root);
        assert_eq!(home.dir, root.join("prova"));
    }

    #[test]
    fn discovery_from_inside_hidden_home_still_roots_at_parent() {
        let t = Tmp::new("inside-hidden");
        t.write(".prova/prova.toml", "[run]\npaths=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&root.join(".prova")).unwrap().unwrap();
        assert_eq!(home.root, root);
        assert_eq!(home.dir, root.join(".prova"));
    }

    #[test]
    fn discovery_walks_up_from_a_subdirectory() {
        let t = Tmp::new("walkup");
        t.write("prova.toml", "[run]\npaths=[\".\"]\n");
        t.write("sub/deep/keep.txt", "x");
        let home = find(&t.0.join("sub/deep")).unwrap().unwrap();
        assert_eq!(home.dir, t.0.canonicalize().unwrap());
    }

    #[test]
    fn two_manifests_in_one_dir_is_ambiguous() {
        let t = Tmp::new("ambiguous");
        t.write("prova.toml", "[run]\npaths=[\".\"]\n");
        t.write(".prova/prova.toml", "[run]\npaths=[\".\"]\n");
        let err = find(&t.0).unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");
        assert!(
            err.contains("prova.toml") && err.contains(".prova/prova.toml"),
            "{err}"
        );
    }

    #[test]
    fn no_manifest_anywhere_is_none() {
        let t = Tmp::new("none");
        t.write("just/files.txt", "x");
        // Walk up from the isolated temp dir; there is no prova.toml in it. (A stray manifest in a
        // real ancestor of the system temp dir is implausible.)
        assert!(find(&t.0.join("just")).unwrap().is_none());
    }
}
