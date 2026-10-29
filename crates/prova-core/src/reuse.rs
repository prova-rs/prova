//! Input-keyed reuse — a verdict carried across trees when what it reads is unchanged
//! (docs/plans/resume.md#phase-2).
//!
//! `--resume` keys a verdict on the WHOLE tree, so any move of `main` re-runs every proof. A leaf
//! that declares `inputs = { … }` narrows its own key to what it reads: the key is a digest of its
//! declared inputs' bytes (root-relative, so two checkouts of the same bytes agree), its declaring
//! file, its qualified path and the prova version. A PASS is filed under that key; under `--reuse`
//! a later run whose key matches carries it forward as reused-by-inputs, naming the run that
//! executed it.
//!
//! This is opt-in by construction — the line incremental-prova.md drew ("cache conducts, never
//! proofs") still holds for every leaf that says nothing: an undeclared leaf always executes. The
//! declaration is the author's claim that the verdict reads nothing else, and the record says the
//! verdict was carried on that claim rather than pretending it ran.
//!
//! The STORE is one small JSON file per key under the state directory: written to a temp name and
//! renamed (a killed run leaves no torn entry), only ever for a pass, pruned by age.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// The key's version tag: bump it whenever what a key covers changes, so no old entry can match.
pub const KEY_VERSION: &str = "inputs-v1";

/// Entries older than this are pruned when a run records: a store that only grows is a disk leak,
/// and a pass this old is a poor warrant anyway.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What a run hands the engine for input-keyed reuse: where the store lives and whether this run
/// may CARRY from it (`--reuse`). Keys are computed whenever a store exists, so a plain run seeds
/// what a later `--reuse` may carry.
#[derive(Debug, Clone)]
pub struct InputReuse {
    pub store: PathBuf,
    pub carry: bool,
}

/// One filed pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub schema: u32,
    /// The file-qualified leaf path, checked again on lookup: a key names one leaf.
    pub path: String,
    /// The run that EXECUTED the pass — never a run that itself reused it.
    pub run_id: String,
    /// RFC 3339, for the person reading the store.
    pub at: String,
}

/// A leaf's reuse key: length-prefixed parts, so no two different questions share one byte stream.
///
/// The prova VERSION, not the binary fingerprint: the fingerprint carries the executable's path and
/// mtime, so it would never match across machines or checkouts — which is the whole point here.
pub fn leaf_key(qualified_path: &str, file_rel: &str, file_digest: &str, inputs_digest: &str) -> String {
    crate::modules::digest_of_parts(&[
        KEY_VERSION,
        qualified_path,
        file_rel,
        file_digest,
        env!("CARGO_PKG_VERSION"),
        inputs_digest,
    ])
}

/// The root-relative digest of a leaf's declared inputs (see `modules::digest_paths_relative`).
pub fn inputs_digest(patterns: &[String], root: &Path) -> String {
    crate::modules::digest_paths_relative(patterns, root)
}

/// The digest of one file's bytes (a declaring `.prova.lua`). A file that cannot be read digests as
/// its absence, which matches no stored pass.
pub fn file_digest(path: &Path) -> String {
    use sha2::Digest as _;
    match std::fs::read(path) {
        Ok(bytes) => hex::encode(sha2::Sha256::digest(bytes)),
        Err(_) => "unreadable".to_string(),
    }
}

fn entry_path(store: &Path, key: &str) -> PathBuf {
    store.join(format!("{key}.json"))
}

/// The pass filed under `key` for `path`, if any. An entry for another path (a collision, a
/// hand-edit) or one that does not parse is no pass.
pub fn lookup(store: &Path, key: &str, path: &str) -> Option<Entry> {
    let text = std::fs::read_to_string(entry_path(store, key)).ok()?;
    let entry: Entry = serde_json::from_str(&text).ok()?;
    (entry.path == path && entry.schema == 1).then_some(entry)
}

/// File a PASS under `key`. Written whole to a temp name and renamed, so a reader never sees half
/// an entry. Overwrites: a later execution of the same question is the fresher witness.
pub fn record(store: &Path, key: &str, entry: &Entry) -> std::io::Result<()> {
    std::fs::create_dir_all(store)?;
    let text = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    let tmp = store.join(format!(".{key}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, entry_path(store, key))
}

/// The selection: of the leaves whose keys this run computed (path → key), those with a filed pass
/// — path → the run that executed it. Paths already carried another way (`--resume`, the stronger
/// warrant) are left to that way, so a reused leaf is attributed to exactly one kind.
pub fn carried(
    store: &Path,
    keys: &BTreeMap<String, String>,
    already: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    keys.iter()
        .filter(|(path, _)| !already.contains(*path))
        .filter_map(|(path, key)| lookup(store, key, path).map(|e| (path.clone(), e.run_id)))
        .collect()
}

/// File every leaf of this run that EXECUTED and PASSED (`passed`), under its key. A failure, a
/// skip, a promise, or a leaf carried rather than executed is never filed: reuse only ever carries
/// a pass forward, and only from the run that earned it. Returns how many were filed.
pub fn file_passes(
    store: &Path,
    keys: &BTreeMap<String, String>,
    passed: &BTreeSet<String>,
    run_id: &str,
) -> std::io::Result<usize> {
    let at = humantime::format_rfc3339_seconds(SystemTime::now()).to_string();
    let mut n = 0;
    for (path, key) in keys.iter().filter(|(p, _)| passed.contains(*p)) {
        record(
            store,
            key,
            &Entry { schema: 1, path: path.clone(), run_id: run_id.to_string(), at: at.clone() },
        )?;
        n += 1;
    }
    if n > 0 {
        prune(store, MAX_AGE);
    }
    Ok(n)
}

/// Drop entries (and stray temp files) older than `max_age`. Best-effort: a prune that fails costs
/// disk, never a verdict.
pub fn prune(store: &Path, max_age: Duration) {
    let Ok(dir) = std::fs::read_dir(store) else { return };
    let now = SystemTime::now();
    for e in dir.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prova-reuse-ut-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn entry(path: &str, run: &str) -> Entry {
        Entry { schema: 1, path: path.into(), run_id: run.into(), at: "2026-10-29T00:00:00Z".into() }
    }

    /// Every part of the key is part of the question: change any one and no stored pass matches.
    #[test]
    fn every_part_of_the_key_changes_it() {
        let base = leaf_key("f › a", "proofs/f.prova.lua", "fd", "id");
        assert_eq!(base, leaf_key("f › a", "proofs/f.prova.lua", "fd", "id"), "deterministic");
        for other in [
            leaf_key("f › b", "proofs/f.prova.lua", "fd", "id"),
            leaf_key("f › a", "proofs/g.prova.lua", "fd", "id"),
            leaf_key("f › a", "proofs/f.prova.lua", "fd2", "id"),
            leaf_key("f › a", "proofs/f.prova.lua", "fd", "id2"),
        ] {
            assert_ne!(base, other);
        }
    }

    /// Two checkouts of the same bytes at different places compute the same key — the property
    /// that lets a gate on a moved `main` carry a pass from the checkout that earned it.
    #[test]
    fn the_same_bytes_at_two_locations_share_a_key() {
        let mk = |tag: &str| {
            let d = store_in(tag);
            std::fs::create_dir_all(d.join("src")).unwrap();
            std::fs::write(d.join("src/a.txt"), "alpha").unwrap();
            std::fs::write(d.join("f.prova.lua"), "prova.test(...)").unwrap();
            d
        };
        let (one, two) = (mk("loc1"), mk("loc2"));
        let key = |d: &Path| {
            leaf_key(
                "f › a",
                "f.prova.lua",
                &file_digest(&d.join("f.prova.lua")),
                &inputs_digest(&["src/a.txt".to_string()], d),
            )
        };
        assert_eq!(key(&one), key(&two));
        std::fs::write(two.join("src/a.txt"), "beta").unwrap();
        assert_ne!(key(&one), key(&two), "an input edit is a different question");
        std::fs::write(two.join("src/a.txt"), "alpha").unwrap();
        std::fs::write(two.join("f.prova.lua"), "prova.test(... edited)").unwrap();
        assert_ne!(key(&one), key(&two), "so is an edit to the declaring file");
        let _ = std::fs::remove_dir_all(&one);
        let _ = std::fs::remove_dir_all(&two);
    }

    /// A filed pass is found under its key for its own path, and nowhere else.
    #[test]
    fn a_recorded_pass_is_found_only_under_its_key_and_path() {
        let store = store_in("roundtrip");
        assert!(lookup(&store, "k1", "f › a").is_none(), "an empty store lends nothing");
        record(&store, "k1", &entry("f › a", "run-1")).unwrap();
        assert_eq!(lookup(&store, "k1", "f › a").unwrap().run_id, "run-1");
        assert!(lookup(&store, "k2", "f › a").is_none(), "another key is another question");
        assert!(lookup(&store, "k1", "f › b").is_none(), "a key names one leaf");
        record(&store, "k1", &entry("f › a", "run-2")).unwrap();
        assert_eq!(lookup(&store, "k1", "f › a").unwrap().run_id, "run-2", "the fresher witness");
        std::fs::write(entry_path(&store, "k3"), "{\"schema\":1,\"pa").unwrap();
        assert!(lookup(&store, "k3", "f › a").is_none(), "a torn entry is no pass");
        let _ = std::fs::remove_dir_all(&store);
    }

    /// Only an executed PASS is filed — the failure beside it is not — and the selection then
    /// carries exactly the filed one, leaving a path already carried by `--resume` to that kind.
    #[test]
    fn a_pass_is_filed_a_failure_is_not_and_only_filed_passes_are_carried() {
        let store = store_in("select");
        let keys: BTreeMap<String, String> =
            [("f › ok", "k-ok"), ("f › red", "k-red"), ("f › res", "k-res")]
                .into_iter()
                .map(|(p, k)| (p.to_string(), k.to_string()))
                .collect();
        let passed: BTreeSet<String> = ["f › ok".to_string(), "f › res".to_string()].into();
        assert_eq!(file_passes(&store, &keys, &passed, "run-1").unwrap(), 2);
        assert!(lookup(&store, "k-red", "f › red").is_none(), "a failure is never reusable");

        let resumed: BTreeSet<String> = ["f › res".to_string()].into();
        let got = carried(&store, &keys, &resumed);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got["f › ok"], "run-1", "carried, naming the run that executed it");

        let mut moved = keys.clone();
        moved.insert("f › ok".into(), "k-ok-after-an-input-edit".into());
        let after = carried(&store, &moved, &BTreeSet::new());
        assert!(!after.contains_key("f › ok"), "a new key carries nothing");
        let _ = std::fs::remove_dir_all(&store);
    }

    /// Pruning drops what is older than the bound and keeps the rest.
    #[test]
    fn prune_drops_only_old_entries() {
        let store = store_in("prune");
        record(&store, "fresh", &entry("f › a", "run-1")).unwrap();
        prune(&store, MAX_AGE);
        assert!(lookup(&store, "fresh", "f › a").is_some(), "a fresh entry survives");
        prune(&store, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        prune(&store, Duration::ZERO);
        assert!(lookup(&store, "fresh", "f › a").is_none(), "an expired one goes");
        let _ = std::fs::remove_dir_all(&store);
    }
}
