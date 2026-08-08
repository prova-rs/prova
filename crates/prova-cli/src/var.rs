//! Where prova's own generated state lives — the `--last-failed` record, held-topology run-state,
//! and anything variable added later.
//!
//! One rule: **prova writes generated state only into a directory it owns, and that directory ignores
//! itself.** The default is `<home>/.prova/var/`, created on the first state WRITE with a `.gitignore`
//! of `*` inside it. Nothing generated ever lands in the package's tracked tree, so a failing run
//! leaves nothing to accidentally commit and no ignore entry for anyone to hand-maintain.
//!
//! Two properties fall out of putting the ignore file *inside* `var/` rather than at `.prova/` level:
//!
//!   * `.prova/` stays free to hold TRACKED content — in the nested layout it holds the manifest,
//!     `config.lua` and `plugins/`, which an ignore one level up would hide.
//!   * The ignore composes **recursively** with no coordination. Every package ignores its own state
//!     and nobody else's, at any nesting depth, because home resolution stops at the nearest manifest
//!     (see `home`). A nested package run standalone gets its own `var/`; a parent run never creates
//!     one for it, because state is written against the *resolved* home and lazily at that.
//!
//! ## `PROVA_VAR_DIR` — an escape hatch, not a preference
//!
//! Some source trees cannot be written to at all: read-only checkouts, Nix and Bazel sandboxes. For
//! those, `PROVA_VAR_DIR` relocates state wholesale. It is deliberately env-only — no manifest key
//! (that is project shape, gets committed, and would drift into a per-project preference) and no CLI
//! flag (that invites casual use).
//!
//! Prova's whole pitch is reliable, consistent runs — no "works on my machine". An environment knob
//! is in tension with that, so the hatch is fenced by three rules:
//!
//!   1. **Location only, never behavior.** State here is never an input to a test outcome, so moving
//!      it cannot change a result. `proofs/layout/state_dir_test.lua` pins that as an equality
//!      assertion on the reported tally, with and without the override.
//!   2. **Absolute paths only.** A relative override resolves against the current directory, and
//!      prova runs from anywhere inside a package (discovery walks up) — so one setting would put
//!      state in different places depending on where you invoked it. That is exactly the
//!      inconsistency the hatch must not introduce, so it is a hard error, not a warning.
//!   3. **Never invisible.** An active override announces itself on stderr. A silent relocation is
//!      how unexplainable machine differences are born; if state moved, the run says so.
//!
//! It names a state **root**, not a state directory: each package gets its own subdirectory keyed by
//! its canonical home path. Without that keying, one shared cache volume across a monorepo (or across
//! this repo's own jj workspaces, which share a store but not a working copy) would have each
//! package's record silently clobber the last — a worse failure than the churn the default fixes.

use std::path::{Path, PathBuf};

use crate::home::Home;

/// The env var that relocates generated state. Empty is treated as unset, so a caller that must
/// guarantee the default can spell it as `PROVA_VAR_DIR=""` rather than unsetting it.
pub const VAR_DIR_ENV: &str = "PROVA_VAR_DIR";

/// The state root override, validated. `Ok(None)` means unset (or empty — see [`VAR_DIR_ENV`]);
/// `Err` carries a ready-to-print diagnostic for a relative path.
pub fn override_root() -> Result<Option<PathBuf>, String> {
    let raw = match std::env::var(VAR_DIR_ENV) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(&raw);
    if !path.is_absolute() {
        return Err(format!(
            "{VAR_DIR_ENV} must be an absolute path (got {raw:?}): a relative override resolves \
             against the current directory, and prova runs from anywhere inside a package — so one \
             setting would put state in different places depending on where you ran it"
        ));
    }
    Ok(Some(path))
}

/// Validate the override once, up front, for EVERY invocation — not lazily at the first state write.
/// A misconfigured hatch must fail the same way on a run that happens to record nothing as on one
/// that does; discovering it only sometimes is its own kind of "works on my machine".
///
/// Returns the diagnostic to print and exit non-zero on.
pub fn check_env() -> Result<(), String> {
    override_root().map(|_| ())
}

/// Announce an active override on stderr, once. stdout stays byte-identical to an un-overridden run
/// so `--format json`/`tap` consumers are unaffected.
pub fn announce() {
    if let Ok(Some(root)) = override_root() {
        eprintln!(
            "prova: state directory overridden by {VAR_DIR_ENV} → {}",
            root.display()
        );
    }
}

/// FNV-1a over the canonical home path. Hand-rolled rather than `DefaultHasher` because this value
/// names a directory prova must find again later: `DefaultHasher`'s output is explicitly not stable
/// across Rust releases, so a toolchain bump would silently orphan every package's state.
fn path_key(path: &Path) -> String {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("package");
    format!("{name}-{hash:016x}")
}

/// This package's state directory, *resolved but not created* — `<home>/.prova/var/`, or a
/// package-keyed subdirectory of `PROVA_VAR_DIR` when the hatch is open.
pub fn path(home: &Home) -> PathBuf {
    match override_root() {
        Ok(Some(root)) => {
            // Key on the CANONICAL home so two spellings of one package (a symlinked checkout, a
            // trailing-slash argument) share one state dir rather than silently forking it.
            let canonical = home.dir.canonicalize().unwrap_or_else(|_| home.dir.clone());
            root.join(path_key(&canonical))
        }
        // A relative override is refused up front by `check_env`; treat it as absent here rather
        // than duplicating the diagnostic on a path that can no longer be reached.
        _ => home.dir.join(".prova").join("var"),
    }
}

/// This package's state directory, created on demand with its self-ignoring `.gitignore`.
///
/// Call this from WRITE paths only. Read paths use [`path`], so an enumeration (`prova promises`,
/// `--help`, a plugin directory with no proofs) never leaves a directory behind.
pub fn dir(home: &Home) -> std::io::Result<PathBuf> {
    let d = path(home);
    std::fs::create_dir_all(&d)?;
    let gitignore = d.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(
            gitignore,
            "# generated by prova — variable run state, never tracked\n*\n",
        )?;
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_at(dir: &str) -> Home {
        Home {
            dir: PathBuf::from(dir),
            manifest: PathBuf::from(dir).join("prova.toml"),
        }
    }

    // The default location: inside the package's own `.prova/`, one level down in `var/` so the
    // ignore cannot reach tracked content in a nested-layout `.prova/`.
    #[test]
    fn default_path_is_under_the_package_nook() {
        let home = home_at("/tmp/pkg");
        assert_eq!(path(&home), PathBuf::from("/tmp/pkg/.prova/var"));
    }

    // Keying is what makes one shared root usable by many packages: different homes, different dirs.
    #[test]
    fn distinct_homes_key_to_distinct_dirs() {
        assert_ne!(
            path_key(Path::new("/a/orders")),
            path_key(Path::new("/b/orders")),
        );
    }

    // ...and stable for one home, or `--last-failed` could never find what the last run wrote.
    #[test]
    fn keying_is_stable_for_one_home() {
        assert_eq!(
            path_key(Path::new("/a/orders")),
            path_key(Path::new("/a/orders")),
        );
    }

    // The key stays legible: a human debugging a shared state root can tell whose dir is whose.
    #[test]
    fn key_carries_the_package_basename() {
        assert!(path_key(Path::new("/work/checkout/orders")).starts_with("orders-"));
    }
}
