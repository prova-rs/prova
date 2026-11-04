//! The conduct cache — a deputy's account carried across trees when the inputs its conduct is built
//! from are unchanged (docs/plans/module-proofs.md#3-cache-the-conduct-never-the-proof).
//!
//! The line incremental-prova.md drew holds exactly: a PROOF is never cached. What costs is the
//! conduct (nextest's build and run of one module's tests), and its inputs are nameable by the
//! build tool. An adapter (`prova.cargo`) computes a module's key from its closure, files the
//! module's account after a conduct in which every case passed, and serves a later run from it,
//! each case marked with the run that executed it. Proofs that read the account always execute.
//!
//! The STORE is one directory per key: the filed files plus `filed.json`, written under a temp name
//! and renamed (a killed run leaves no torn entry), pruned by age. Under `PROVA_VAR_DIR` it sits at
//! the override root, so a fresh checkout (Herald's) is served what a lane's run filed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// The key's version tag: bump it whenever what a key covers changes, so no old entry can match.
pub const KEY_VERSION: &str = "conducts-v1";

/// What a run hands the engine: where the store lives, whether lookups are bypassed (`--uncached`:
/// the audit run conducts everything), and, when this run may not file, why.
#[derive(Debug, Clone)]
pub struct ConductCache {
    pub store: PathBuf,
    pub fresh: bool,
    /// `Some(why)`: this run's conducts are not the conduct a later run asks about (`--falsify`,
    /// `--update-baseline`, `-u`, a `--dep` override), so nothing is filed, and `file` says why.
    pub unfileable: Option<String>,
}

/// One filed account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filed {
    pub schema: u32,
    /// The run that EXECUTED the conduct — never a run that was itself served from the cache.
    pub run_id: String,
    /// RFC 3339, for the person reading the store.
    pub at: String,
    /// The filed files, by the name the adapter gave each (`junit` → `junit.xml`).
    pub files: BTreeMap<String, String>,
}

/// A conduct's key: length-prefixed `parts` (the adapter's name, the module, anything else that
/// decides its conduct), the prova version, and the root-relative digest of `paths` (its closure:
/// each matched file's relative path, length and bytes), so two checkouts of the same bytes agree.
pub fn key(parts: &[String], paths: &[String], root: &Path) -> String {
    let inputs = crate::modules::digest_paths_relative(paths, root);
    let mut all: Vec<&str> = vec![KEY_VERSION, env!("CARGO_PKG_VERSION")];
    all.extend(parts.iter().map(String::as_str));
    all.push(&inputs);
    crate::modules::digest_of_parts(&all)
}

impl ConductCache {
    fn dir(&self, key: &str) -> PathBuf {
        self.store.join(key)
    }

    /// The account filed under `key` and the directory its files are in, unless this run is fresh.
    pub fn lookup(&self, key: &str) -> Option<(Filed, PathBuf)> {
        if self.fresh {
            return None;
        }
        let dir = self.dir(key);
        let text = std::fs::read_to_string(dir.join("filed.json")).ok()?;
        let filed: Filed = serde_json::from_str(&text).ok()?;
        let whole = filed.schema == 1 && filed.files.values().all(|f| dir.join(f).is_file());
        whole.then_some((filed, dir))
    }

    /// File `files` (name → a path this run's conduct wrote) under `key`, as executed by `run_id`.
    /// Copied, never referenced: a conduct's output lies under a target directory the next build
    /// rewrites. Overwrites: a later execution of the same conduct is the fresher witness.
    pub fn file(&self, key: &str, files: &BTreeMap<String, PathBuf>, run_id: &str) -> Result<PathBuf, String> {
        if let Some(why) = &self.unfileable {
            return Err(format!("this run files no conduct: {why}"));
        }
        if files.is_empty() {
            return Err("a filed conduct names at least one file".into());
        }
        let tmp = self.store.join(format!(".{key}.{}.tmp", std::process::id()));
        let io = |what: &str, p: &Path, e: std::io::Error| format!("{what} {}: {e}", p.display());
        drop(std::fs::remove_dir_all(&tmp));
        std::fs::create_dir_all(&tmp).map_err(|e| io("cannot create", &tmp, e))?;
        let mut names = BTreeMap::new();
        for (name, from) in files {
            let Some(file_name) = from.file_name() else {
                return Err(format!("{name}: {} names no file", from.display()));
            };
            let to = tmp.join(file_name);
            std::fs::copy(from, &to).map_err(|e| io("cannot copy", from, e))?;
            names.insert(name.clone(), file_name.to_string_lossy().into_owned());
        }
        let filed = Filed {
            schema: 1,
            run_id: run_id.to_string(),
            at: humantime::format_rfc3339_seconds(SystemTime::now()).to_string(),
            files: names,
        };
        let text = serde_json::to_string_pretty(&filed).map_err(|e| e.to_string())?;
        std::fs::write(tmp.join("filed.json"), text).map_err(|e| io("cannot write", &tmp, e))?;
        let dir = self.dir(key);
        drop(std::fs::remove_dir_all(&dir));
        std::fs::rename(&tmp, &dir).map_err(|e| io("cannot file", &dir, e))?;
        prune(&self.store);
        Ok(dir)
    }
}

/// Drop filed accounts (and stray temp dirs) older than the reuse store's age. Best-effort: a prune
/// that fails costs disk, never a verdict.
fn prune(store: &Path) {
    let Ok(dir) = std::fs::read_dir(store) else { return };
    let now = SystemTime::now();
    for e in dir.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > crate::reuse::MAX_AGE);
        if old {
            drop(std::fs::remove_dir_all(e.path()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prova-conducts-ut-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn cache(tag: &str, fresh: bool, unfileable: Option<&str>) -> (ConductCache, PathBuf) {
        let dir = scratch(tag);
        let c = ConductCache { store: dir.join("conducts"), fresh, unfileable: unfileable.map(str::to_owned) };
        (c, dir)
    }

    fn artifact(dir: &Path, body: &str) -> BTreeMap<String, PathBuf> {
        let p = dir.join("junit.xml");
        std::fs::write(&p, body).expect("write");
        BTreeMap::from([("junit".to_string(), p)])
    }

    /// A filed account is served under its key, names the run that executed it, and is a copy
    /// that outlives the conduct's own file.
    #[test]
    fn a_filed_account_is_served_by_its_key_and_outlives_the_conducts_file() {
        let (c, dir) = cache("served", false, None);
        let files = artifact(&dir, "<testsuites/>");
        c.file("k1", &files, "run-a").expect("filed");
        std::fs::remove_file(&files["junit"]).expect("the conduct's file goes");
        let (filed, at) = c.lookup("k1").expect("served");
        assert_eq!(filed.run_id, "run-a");
        assert_eq!(std::fs::read_to_string(at.join(&filed.files["junit"])).expect("copy"), "<testsuites/>");
        assert!(c.lookup("k2").is_none(), "another key is another conduct");
    }

    /// `--uncached` serves nothing (the audit conducts everything) but still files what it executed.
    #[test]
    fn a_fresh_run_is_served_nothing_and_still_files() {
        let (c, dir) = cache("fresh", true, None);
        c.file("k1", &artifact(&dir, "x"), "run-fresh").expect("a fresh run files");
        assert!(c.lookup("k1").is_none(), "fresh: never served");
        let served = ConductCache { fresh: false, ..c };
        assert_eq!(served.lookup("k1").expect("filed").0.run_id, "run-fresh");
    }

    /// A run that may not file says why, and files nothing.
    #[test]
    fn a_run_that_may_not_file_says_why_and_files_nothing() {
        let (c, dir) = cache("refused", false, Some("--falsify executes mutants"));
        let err = c.file("k1", &artifact(&dir, "x"), "run").expect_err("refused");
        assert!(err.contains("--falsify"), "{err}");
        assert!(c.lookup("k1").is_none());
    }

    /// The key moves with every part, the prova version included, and with the closure's bytes;
    /// two checkouts of the same bytes share it.
    #[test]
    fn the_key_moves_with_parts_and_bytes_and_not_with_location() {
        let (a, b) = (scratch("key-a"), scratch("key-b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d.join("src")).expect("dir");
            std::fs::write(d.join("src/lib.rs"), "fn x() {}").expect("write");
        }
        let parts = vec!["cargo".to_string(), "scm".to_string()];
        let paths = vec!["src/lib.rs".to_string()];
        let ka = key(&parts, &paths, &a);
        assert_eq!(ka, key(&parts, &paths, &b), "same bytes, another checkout: one key");
        assert_ne!(ka, key(&["cargo".into(), "graph".into()], &paths, &a), "another module");
        std::fs::write(b.join("src/lib.rs"), "fn y() {}").expect("edit");
        assert_ne!(ka, key(&parts, &paths, &b), "an edit in the closure moves the key");
    }
}
