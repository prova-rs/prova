//! Locating the "prova home" — the directory that owns a project's `prova.toml`, and against which
//! every relative path in the manifest (`[run] paths`, `config`, `plugin_root`) and generated
//! artifact (`.luarc.json`, `running/`) resolves.
//!
//! **The manifest's directory is the base for everything — there is no separate "root".** A project
//! may keep its manifest in one of four places:
//!
//! | Location | Home dir | Feel |
//! |---|---|---|
//! | `prova.toml` | the dir holding it | flat — prova at the root, zero nesting |
//! | `.prova.toml` | the dir holding it | flat, hidden — one manifest, out of sight |
//! | `prova/prova.toml` | `prova/` | visible nested — a self-contained `prova/` dir |
//! | `.prova/prova.toml` | `.prova/` | hidden nested — a self-contained `.prova/` dir |
//!
//! Because home *is* the manifest's directory, a project is a **relocatable unit**: move the manifest
//! and the files it references from `prova/` up to the root, and the manifest does not change a byte
//! — every path in it was always relative to wherever the manifest sits.
//!
//! Discovery walks **up** from the current directory (like git finding `.git`), so `prova` works from
//! anywhere inside a project, and the **nearest** manifest wins — a deeper `prova.toml` is its own
//! independent project, not a child of an ancestor's. Finding **more than one** of the four variants
//! in a *single* directory is a hard error: that layout is ambiguous and prova refuses to guess.

use std::path::{Path, PathBuf};

/// A located prova home: the directory the manifest lives in, and the manifest file itself. Every
/// manifest-relative path and generated artifact resolves against `dir` — it is both the "root" and
/// the "home" the old two-field model split, now unified.
#[derive(Debug, Clone, PartialEq)]
pub struct Home {
    /// The home directory — where the manifest lives (a flat root, or a `prova/` / `.prova/` child).
    /// The single base for every manifest-relative path (`paths`, `config`, `plugin_root`) and for
    /// `running/` state.
    pub dir: PathBuf,
    /// The manifest file inside `dir`.
    pub manifest: PathBuf,
}

impl Home {
    /// The directory an editor opens as the workspace — where `.luarc.json` (and `.claude/skills/`)
    /// belong. It is the home dir for a flat manifest, and the **parent** of a nested `prova/` /
    /// `.prova/` home (you open `myproject/`, never `myproject/.prova/`).
    ///
    /// This is the *only* thing that does not resolve against the home dir, and deliberately so: an
    /// editor artifact's location is governed by where the editor attaches, not by where the manifest
    /// resolves paths.
    ///
    /// The disambiguator is the manifest **filename**, not the directory name alone. Only a *bare*
    /// `prova.toml` inside a directory named `prova`/`.prova` is a nested home (root = the parent) —
    /// which reserves those two directory names for the purpose. A hidden `.prova.toml` is a flat
    /// file whatever its directory is called, so it never hoists. The lone irreducible case — a bare
    /// `prova.toml` in a dir named `prova`/`.prova`, indistinguishable on disk from a nested home — is
    /// resolved by that reservation; to root a *flat* project in such a directory, use `.prova.toml`.
    pub fn editor_root(&self) -> PathBuf {
        let nested = self.manifest.file_name().and_then(|s| s.to_str()) == Some("prova.toml")
            && matches!(
                self.dir.file_name().and_then(|s| s.to_str()),
                Some("prova" | ".prova")
            );
        if nested {
            self.dir.parent().unwrap_or(&self.dir).to_path_buf()
        } else {
            self.dir.clone()
        }
    }
}

/// Walk up from `start`, returning the first ancestor that holds a manifest. `Ok(None)` means no
/// manifest anywhere up to the filesystem root (an ad-hoc invocation with no project). `Err` means a
/// single directory holds more than one of the four variants (ambiguous — keep exactly one).
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

/// Build a `Home` from an explicit manifest path (`--manifest`): home is the manifest's own directory.
pub fn from_manifest_path(manifest: &Path) -> Home {
    Home {
        dir: manifest.parent().unwrap_or(Path::new(".")).to_path_buf(),
        manifest: manifest.to_path_buf(),
    }
}

/// A manifest variant discovered at some directory level: its display label and the (home dir,
/// manifest file) it implies.
struct Candidate {
    label: &'static str,
    dir: PathBuf,
    manifest: PathBuf,
}

/// Check directory `dir` for the four manifest variants. `Ok(None)` if none present; `Ok(Some)` for
/// exactly one; `Err` if more than one (ambiguous layout).
///
/// Home is always the directory the manifest sits in: `dir` for the two flat variants, `dir/prova` or
/// `dir/.prova` for the two nested ones. No special-casing of directory names is needed — running
/// from inside `prova/` finds the flat `prova/prova.toml` (home `prova/`), and running from the parent
/// finds the nested `prova/prova.toml` (home also `prova/`), so both agree without a rule.
fn at(dir: &Path) -> Result<Option<Home>, String> {
    let variants = [
        ("prova.toml", dir.join("prova.toml"), dir.to_path_buf()),
        (".prova.toml", dir.join(".prova.toml"), dir.to_path_buf()),
        (
            "prova/prova.toml",
            dir.join("prova").join("prova.toml"),
            dir.join("prova"),
        ),
        (
            ".prova/prova.toml",
            dir.join(".prova").join("prova.toml"),
            dir.join(".prova"),
        ),
    ];

    let found: Vec<Candidate> = variants
        .into_iter()
        .filter(|(_, manifest, _)| manifest.is_file())
        .map(|(label, manifest, home_dir)| Candidate {
            label,
            dir: home_dir,
            manifest,
        })
        .collect();

    match found.as_slice() {
        [] => Ok(None),
        [c] => Ok(Some(Home {
            dir: c.dir.clone(),
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

    // The two flat variants: home is the directory holding the manifest.
    #[test]
    fn flat_manifest_home_is_its_own_dir() {
        let t = Tmp::new("flat");
        t.write("prova.toml", "[run]\npaths=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root);
        assert_eq!(home.manifest, root.join("prova.toml"));
    }

    #[test]
    fn hidden_flat_manifest_home_is_its_own_dir() {
        let t = Tmp::new("hidden-file");
        t.write(".prova.toml", "[run]\npaths=[\"proofs\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root);
        assert_eq!(home.manifest, root.join(".prova.toml"));
    }

    // The two nested variants: home is the `prova/` or `.prova/` dir itself, NOT its parent.
    #[test]
    fn nested_visible_home_is_the_prova_dir() {
        let t = Tmp::new("visible");
        t.write("prova/prova.toml", "[run]\npaths=[\"suites\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root.join("prova"));
        assert_eq!(home.manifest, root.join("prova/prova.toml"));
    }

    #[test]
    fn nested_hidden_home_is_the_dot_prova_dir() {
        let t = Tmp::new("hidden");
        t.write(".prova/prova.toml", "[run]\npaths=[\"suites\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root.join(".prova"));
    }

    // Discovery is stable: running from inside `prova/` resolves the SAME home as from the parent,
    // and needs no directory-name special-casing to do it.
    #[test]
    fn discovery_from_inside_nested_home_is_stable() {
        let t = Tmp::new("inside-visible");
        t.write("prova/prova.toml", "[run]\npaths=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let from_parent = find(&t.0).unwrap().unwrap();
        let from_inside = find(&root.join("prova")).unwrap().unwrap();
        assert_eq!(from_parent, from_inside);
        assert_eq!(from_inside.dir, root.join("prova"));
    }

    #[test]
    fn discovery_walks_up_from_a_subdirectory() {
        let t = Tmp::new("walkup");
        t.write("prova.toml", "[run]\npaths=[\".\"]\n");
        t.write("sub/deep/keep.txt", "x");
        let home = find(&t.0.join("sub/deep")).unwrap().unwrap();
        assert_eq!(home.dir, t.0.canonicalize().unwrap());
    }

    // A deeper manifest is its own project — the nearest wins, and this is NOT the same-directory
    // ambiguity that is an error.
    #[test]
    fn nearest_manifest_wins_deeper_is_independent() {
        let t = Tmp::new("nested-projects");
        t.write("prova.toml", "[run]\npaths=[\".\"]\n");
        t.write("sub/prova.toml", "[run]\npaths=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        assert_eq!(find(&t.0).unwrap().unwrap().dir, root);
        assert_eq!(
            find(&root.join("sub")).unwrap().unwrap().dir,
            root.join("sub")
        );
    }

    #[test]
    fn two_variants_in_one_dir_is_ambiguous() {
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
        assert!(find(&t.0.join("just")).unwrap().is_none());
    }

    // The editor root (where `.luarc.json` goes) is the home dir for a flat manifest, and the PARENT
    // of a nested `prova/` / `.prova/` home — the directory you actually open in an editor.
    #[test]
    fn editor_root_is_home_for_flat_and_parent_for_nested() {
        let t = Tmp::new("editor-root");
        t.write("flat/prova.toml", "[run]\npaths=[\".\"]\n");
        t.write("nested/.prova/prova.toml", "[run]\npaths=[\".\"]\n");
        let flat = find(&t.0.join("flat")).unwrap().unwrap();
        let nested = find(&t.0.join("nested")).unwrap().unwrap();

        assert_eq!(flat.editor_root(), flat.dir); // flat: editor root == home
        assert_eq!(
            nested.dir,
            t.0.canonicalize().unwrap().join("nested/.prova")
        );
        assert_eq!(
            nested.editor_root(),
            t.0.canonicalize().unwrap().join("nested")
        ); // parent of .prova
    }

    // The disambiguator is the manifest FILENAME, not the dir name alone. A hidden-flat `.prova.toml`
    // is flat even when its directory is literally named `prova`/`.prova`, so its editor root is that
    // dir, NOT the parent. (Only a bare `prova.toml` inside such a dir is a nested home.)
    #[test]
    fn hidden_flat_in_a_prova_named_dir_roots_at_that_dir() {
        let t = Tmp::new("editor-root-name");
        // A dir literally named `prova`, holding a hidden-flat `.prova.toml` — a flat project.
        t.write("prova/.prova.toml", "[run]\npaths=[\".\"]\n");
        let home = find(&t.0.join("prova")).unwrap().unwrap();
        let dir = t.0.canonicalize().unwrap().join("prova");
        assert_eq!(home.dir, dir);
        // Editor root is the project dir itself — keying on the dir name alone would wrongly hoist
        // this to the parent.
        assert_eq!(home.editor_root(), dir);
    }
}
