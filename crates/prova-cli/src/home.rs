//! Locating the "prova home" — the PROJECT ROOT that owns a package's manifest, and against which
//! every relative path in the manifest (`[run] proofs`, `config`, `plugin_root`) and generated
//! artifact (`.luarc.json`, `running/`) resolves.
//!
//! A package keeps its manifest in one of four places; the home is the project **root** in every case:
//!
//! | Location | Home (root) | Feel |
//! |---|---|---|
//! | `prova.toml` | the dir holding it | flat — prova at the root, zero nesting |
//! | `.prova.toml` | the dir holding it | flat, hidden — one manifest, out of sight |
//! | `prova/prova.toml` | the dir **above** `prova/` | visible nested — config tucked in `prova/` |
//! | `.prova/prova.toml` | the dir **above** `.prova/` | hidden nested — config tucked in `.prova/` |
//!
//! The nested forms let a package tuck prova's own files (the manifest, `config.lua`, `plugins/`) into
//! a `prova/` or `.prova/` directory while the ROOT — where `proofs/` live and where an editor
//! attaches — stays the parent. So `config = ".prova/config.lua"` and `proofs = ["proofs"]` in a
//! `.prova/prova.toml` resolve to `<root>/.prova/config.lua` and (via discovery) `<root>/proofs`:
//! everything is relative to the root, never to the manifest's own directory.
//!
//! Discovery walks **up** from the current directory (like git finding `.git`), so `prova` works from
//! anywhere inside a package — including from inside the `.prova/` nook itself — and the **nearest**
//! manifest wins. The disambiguator for a bare `prova.toml` is the directory NAME: a bare `prova.toml`
//! in a directory named `prova`/`.prova` is a nested manifest (home = the parent), so discovery from
//! inside the nook agrees with discovery from the root. To root a *flat* package in a directory that
//! happens to be named `prova`/`.prova`, use the hidden `.prova.toml`, which is always flat. Finding
//! more than one variant in a single directory is a hard error: ambiguous, and prova refuses to guess.

use std::path::{Path, PathBuf};

/// A located prova home: the project ROOT, and the manifest file that roots it. Every manifest-relative
/// path and generated artifact resolves against `dir` (the root); `manifest` is the file itself, which
/// for a nested layout lives in a `prova/` or `.prova/` child of `dir`.
#[derive(Debug, Clone, PartialEq)]
pub struct Home {
    /// The home directory — the PROJECT ROOT. The single base for every manifest-relative path
    /// (`proofs`, `config`, `plugin_root`), for generated `.luarc.json` / `running/` state, and the
    /// directory an editor attaches to.
    pub dir: PathBuf,
    /// The manifest file. For a flat layout it sits directly in `dir`; for a nested layout it sits in
    /// `dir/prova/` or `dir/.prova/`.
    pub manifest: PathBuf,
}

/// Whether `dir` is a nested nook — a directory named `prova` or `.prova`. A bare `prova.toml` found
/// *in* such a directory is a nested manifest (home = the parent); the name is the disambiguator, so
/// those two directory names are reserved for the nested layout.
fn dir_is_nook(dir: &Path) -> bool {
    matches!(
        dir.file_name().and_then(|s| s.to_str()),
        Some("prova" | ".prova")
    )
}

/// Whether `dir` is itself a package root — it holds any of the four manifest variants. Proof
/// discovery uses this to stop at a nested package boundary: a deeper package is independent, not part
/// of this one (the same "nearest manifest wins" rule `find`/`at` apply when walking up).
pub fn has_manifest(dir: &Path) -> bool {
    dir.join("prova.toml").is_file()
        || dir.join(".prova.toml").is_file()
        || dir.join("prova").join("prova.toml").is_file()
        || dir.join(".prova").join("prova.toml").is_file()
}

/// Walk up from `start`, returning the first ancestor that holds a manifest. `Ok(None)` means no
/// manifest anywhere up to the filesystem root (an ad-hoc invocation with no package). `Err` means a
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

/// Build a `Home` from an explicit manifest path (`--manifest`). The root is the manifest's directory,
/// hoisted to the parent when the manifest is a nested `prova/prova.toml` / `.prova/prova.toml`.
pub fn from_manifest_path(manifest: &Path) -> Home {
    // A bare relative name like `prova.toml` has parent `""` — normalize to `.` or every
    // root-relative walk (proof discovery above all) scans the empty path and finds nothing.
    let mdir = match manifest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let bare_prova = manifest.file_name().and_then(|s| s.to_str()) == Some("prova.toml");
    let dir = if bare_prova && dir_is_nook(&mdir) {
        mdir.parent().unwrap_or(&mdir).to_path_buf()
    } else {
        mdir
    };
    Home {
        dir,
        manifest: manifest.to_path_buf(),
    }
}

/// A manifest variant discovered at some directory level: its display label and the (root dir,
/// manifest file) it implies.
struct Candidate {
    label: &'static str,
    dir: PathBuf,
    manifest: PathBuf,
}

/// Check directory `dir` for the four manifest variants. `Ok(None)` if none present; `Ok(Some)` for
/// exactly one; `Err` if more than one (ambiguous layout).
///
/// Home is the project ROOT: `dir` for the two flat variants; the dir **above** `prova/`/`.prova/` for
/// the two nested ones. A bare `prova.toml` found while `dir` itself is a `prova`/`.prova` nook is that
/// same nested manifest seen from the inside — home is `dir`'s parent — so a run from inside the nook
/// resolves the identical home as a run from the root.
fn at(dir: &Path) -> Result<Option<Home>, String> {
    let parent = dir.parent().unwrap_or(dir).to_path_buf();
    // A bare `prova.toml` roots at `dir` normally, but at the parent when `dir` is a nook.
    let bare_root = if dir_is_nook(dir) {
        parent
    } else {
        dir.to_path_buf()
    };
    let variants = [
        ("prova.toml", dir.join("prova.toml"), bare_root),
        (".prova.toml", dir.join(".prova.toml"), dir.to_path_buf()),
        (
            "prova/prova.toml",
            dir.join("prova").join("prova.toml"),
            dir.to_path_buf(),
        ),
        (
            ".prova/prova.toml",
            dir.join(".prova").join("prova.toml"),
            dir.to_path_buf(),
        ),
    ];

    let found: Vec<Candidate> = variants
        .into_iter()
        .filter(|(_, manifest, _)| manifest.is_file())
        .map(|(label, manifest, root)| Candidate {
            label,
            dir: root,
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

    // `--manifest prova.toml` (a bare relative name): the home dir must be `.`, never the empty
    // path — discovery from `""` scans nothing and every proof goes undiscovered.
    #[test]
    fn bare_relative_manifest_path_homes_at_dot() {
        let home = from_manifest_path(Path::new("prova.toml"));
        assert_eq!(home.dir, PathBuf::from("."));
        assert_eq!(home.manifest, PathBuf::from("prova.toml"));
    }

    // The two flat variants: home is the directory holding the manifest.
    #[test]
    fn flat_manifest_home_is_its_own_dir() {
        let t = Tmp::new("flat");
        t.write("prova.toml", "[run]\nproofs=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root);
        assert_eq!(home.manifest, root.join("prova.toml"));
    }

    #[test]
    fn hidden_flat_manifest_home_is_its_own_dir() {
        let t = Tmp::new("hidden-file");
        t.write(".prova.toml", "[run]\nproofs=[\"proofs\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root);
        assert_eq!(home.manifest, root.join(".prova.toml"));
    }

    // The two nested variants: home is the project ROOT — the directory ABOVE `prova/`/`.prova/` — and
    // the manifest file sits inside that nook.
    #[test]
    fn nested_visible_home_is_the_parent_root() {
        let t = Tmp::new("visible");
        t.write("prova/prova.toml", "[run]\nproofs=[\"proofs\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root); // NOT root/prova
        assert_eq!(home.manifest, root.join("prova/prova.toml"));
    }

    #[test]
    fn nested_hidden_home_is_the_parent_root() {
        let t = Tmp::new("hidden");
        t.write(".prova/prova.toml", "[run]\nproofs=[\"proofs\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = find(&t.0).unwrap().unwrap();
        assert_eq!(home.dir, root); // NOT root/.prova
        assert_eq!(home.manifest, root.join(".prova/prova.toml"));
    }

    // Discovery is stable AND agrees on the root: running from inside the nook resolves the SAME home
    // (the parent) as running from the root — the directory-name disambiguation is what makes it so.
    #[test]
    fn discovery_from_inside_nested_nook_resolves_the_root() {
        let t = Tmp::new("inside-visible");
        t.write("prova/prova.toml", "[run]\nproofs=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let from_parent = find(&t.0).unwrap().unwrap();
        let from_inside = find(&root.join("prova")).unwrap().unwrap();
        assert_eq!(from_parent, from_inside);
        assert_eq!(from_inside.dir, root); // the parent, from inside `prova/`
    }

    #[test]
    fn discovery_from_inside_hidden_nook_resolves_the_root() {
        let t = Tmp::new("inside-hidden");
        t.write(".prova/prova.toml", "[run]\nproofs=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let from_inside = find(&root.join(".prova")).unwrap().unwrap();
        assert_eq!(from_inside.dir, root); // the parent, from inside `.prova/`
        assert_eq!(from_inside.manifest, root.join(".prova/prova.toml"));
    }

    #[test]
    fn discovery_walks_up_from_a_subdirectory() {
        let t = Tmp::new("walkup");
        t.write("prova.toml", "[run]\nproofs=[\".\"]\n");
        t.write("sub/deep/keep.txt", "x");
        let home = find(&t.0.join("sub/deep")).unwrap().unwrap();
        assert_eq!(home.dir, t.0.canonicalize().unwrap());
    }

    // A deeper manifest is its own package — the nearest wins, and this is NOT the same-directory
    // ambiguity that is an error.
    #[test]
    fn nearest_manifest_wins_deeper_is_independent() {
        let t = Tmp::new("nested-packages");
        t.write("prova.toml", "[run]\nproofs=[\".\"]\n");
        t.write("sub/prova.toml", "[run]\nproofs=[\".\"]\n");
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
        t.write("prova.toml", "[run]\nproofs=[\".\"]\n");
        t.write(".prova/prova.toml", "[run]\nproofs=[\".\"]\n");
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

    // The disambiguator is the manifest FILENAME, not the dir name alone. A hidden-flat `.prova.toml`
    // is flat even when its directory is literally named `prova`/`.prova`, so it roots at that dir, NOT
    // the parent. (Only a bare `prova.toml` inside such a dir is a nested manifest.)
    #[test]
    fn hidden_flat_in_a_prova_named_dir_roots_at_that_dir() {
        let t = Tmp::new("flat-in-nook");
        t.write("prova/.prova.toml", "[run]\nproofs=[\".\"]\n");
        let home = find(&t.0.join("prova")).unwrap().unwrap();
        let dir = t.0.canonicalize().unwrap().join("prova");
        assert_eq!(home.dir, dir); // flat: roots at the dir itself, not the parent
    }

    // `--manifest` at a nested manifest hoists the root to the parent, exactly like discovery.
    #[test]
    fn explicit_nested_manifest_roots_at_the_parent() {
        let t = Tmp::new("explicit");
        t.write(".prova/prova.toml", "[run]\nproofs=[\".\"]\n");
        let root = t.0.canonicalize().unwrap();
        let home = from_manifest_path(&root.join(".prova/prova.toml"));
        assert_eq!(home.dir, root);
    }
}
