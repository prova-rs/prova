//! Who holds a lock — the sidecar record that makes an anonymous `flock` nameable
//! (docs/plans/lock-starvation.md).
//!
//! A `flock(2)` tells the kernel everything and the operator nothing. When a hung holder pinned
//! this package's `cargo` token for 1 d 22 h (2026-09-01, substrate), every waiter could say was
//! "still waiting" — no pid, no package, no command — and the only diagnostic left was `ps`. So a
//! holder writes a record of itself beside the lock, keyed by pid, and removes it on release.
//!
//! **The lock file's inode must never change, and that is what forces a sidecar.** `flock` binds
//! the open file *description*, which binds an inode. Writing the record INTO `<token>.lock` with
//! the temp-file-plus-rename discipline `runstate` uses would hand every later opener a
//! *different* inode to flock — two processes would both "hold" the token and the mutual
//! exclusion would be silently gone. The lock file therefore stays an empty file nobody writes,
//! and records live in `<token>.holders/<pid>.json` next to it.
//!
//! **The record is a hint; the flock is authority.** Two places recording one fact can disagree
//! (the split docs/design/agent-ergonomics.md#machine-wide-held-topology-index already names for
//! topologies), and here they legitimately do: the lock file is a PUBLIC convention, so an
//! external tool — this repo's own `xtask`, a Makefile, `flock(1)` — can hold the token without
//! knowing this format exists. Held-with-no-record is therefore a real and supported state with a
//! truthful name ("an unregistered holder"), never evidence that the token is free.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// One holder's self-description. Written after the flock is taken, removed before it is released.
///
/// There is deliberately no `progress_at` here yet. The field the waiter actually wants — "is this
/// holder getting anywhere?" — can only be written by a holder that supervises its own conducts,
/// which is Part 1 of the plan; shipping the field now would mean shipping a number nobody
/// produces, and a stale-forever `progress_at` reads as "hung" for every healthy holder
/// (docs/design/agent-ergonomics.md#a-measurement-must-prove-it-measured). It arrives with the
/// supervision that can honestly fill it, behind `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub token: String,
    /// A concurrent (reader) hold. Several of these coexist; at most one exclusive holder does.
    pub shared: bool,
    /// The package this hold was taken from. Machine-scoped holds still name their origin — "who
    /// is holding port 5432" is answered by a path, not by a token.
    pub package: Option<String>,
    /// The holder's own command line. Derived rather than passed by the caller
    /// ([`describe_self`]): a label a caller must remember to supply is a label that goes stale
    /// or missing at exactly the call site nobody thought about.
    pub what: String,
    /// Unix seconds at acquisition.
    pub acquired_at: u64,
}

/// What a holder record can be, read from disk. Dead holders are swept rather than returned — a
/// record whose pid is gone cannot be part of a live hold, since the kernel released that flock
/// the instant the process died.
#[derive(Debug, Clone)]
pub enum Entry {
    /// Parsed, and its process is still alive.
    Live(Holder),
    /// The file is there and the pid (from the filename) is alive, but the JSON does not parse.
    /// Reported rather than dropped, for the reason
    /// docs/design/agent-ergonomics.md#unparseable-runstate-record-reads-as-no-hold spells out:
    /// silently dropping what does not deserialize makes a LIVE holder invisible.
    Unreadable { pid: u32, why: String },
}

impl Entry {
    pub fn pid(&self) -> u32 {
        match self {
            Entry::Live(h) => h.pid,
            Entry::Unreadable { pid, .. } => *pid,
        }
    }
}

/// The records directory for a lock file: `<…>/cargo.lock` → `<…>/cargo.holders`.
pub fn dir_for(lock_path: &Path) -> PathBuf {
    lock_path.with_extension("holders")
}

fn record_path(lock_path: &Path, pid: u32) -> PathBuf {
    dir_for(lock_path).join(format!("{pid}.json"))
}

/// Unix seconds now, or 0 if the clock is before the epoch (which is not a reason to fail a hold).
fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Write this process's record beside `lock_path`, returning the path to remove on release.
///
/// Best-effort by construction: the hold is already taken when this runs, and a package whose
/// `var/` is read-only should still be able to serialize its builds. A failure here costs
/// diagnosability, never correctness — so it returns `None` rather than an error, and the
/// resulting state (`held`, no record) is one the readers below already have a word for.
pub fn register(
    lock_path: &Path,
    token: &str,
    shared: bool,
    package: Option<&Path>,
) -> Option<PathBuf> {
    let dir = dir_for(lock_path);
    std::fs::create_dir_all(&dir).ok()?;
    let record = Holder {
        pid: std::process::id(),
        token: token.to_string(),
        shared,
        package: package.map(|p| p.display().to_string()),
        what: describe_self(),
        acquired_at: now_secs(),
    };
    let path = record_path(lock_path, record.pid);
    // A plain write is right here, and the contrast with `runstate::write` is the point: this
    // file is keyed by pid, so it has exactly one writer and no reader can catch a competing
    // one mid-write. What a reader CAN catch is this process's own crash between create and
    // flush — which is why a truncated record is reported as `Unreadable` rather than trusted.
    std::fs::write(&path, serde_json::to_vec_pretty(&record).ok()?).ok()?;
    Some(path)
}

/// Remove a record written by [`register`]. Failure is ignored deliberately: the flock is about to
/// be released either way, and the leftover file names a pid that will be dead — the sweep in
/// [`read_all`] collects it on the next read.
pub fn unregister(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Every live holder recorded beside `lock_path`, sweeping records whose process is gone.
///
/// Sweeping on read rather than on a schedule follows `barrier`: the pid is in the filename, so a
/// dead holder's record is identifiable without parsing it, and removal is only ever safe in the
/// direction "this process no longer exists".
pub fn read_all(lock_path: &Path) -> Vec<Entry> {
    let dir = dir_for(lock_path);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if !process_is_alive(pid as i32) {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        out.push(match std::fs::read_to_string(entry.path()) {
            Ok(text) => match serde_json::from_str::<Holder>(&text) {
                Ok(h) => Entry::Live(h),
                Err(e) => Entry::Unreadable { pid, why: e.to_string() },
            },
            Err(e) => Entry::Unreadable { pid, why: e.to_string() },
        });
    }
    out.sort_by_key(Entry::pid);
    out
}

/// One line naming who holds `lock_path`, for a waiter's narration and for `prova locks`.
///
/// The no-record case is phrased as a positive fact about an unregistered holder rather than as an
/// absence, because absence is precisely the wrong reading: the flock is held, and the convention
/// invites tools that will never write one of these files.
pub fn describe_holders(lock_path: &Path) -> String {
    let entries = read_all(lock_path);
    if entries.is_empty() {
        return "held by an unregistered holder (no record beside the lock — an external tool \
                joining the convention, or a holder from before this prova)"
            .to_string();
    }
    let now = now_secs();
    let described: Vec<String> = entries
        .iter()
        .map(|e| match e {
            Entry::Live(h) => {
                let held_for = age(Duration::from_secs(now.saturating_sub(h.acquired_at)));
                match &h.package {
                    Some(pkg) => format!("pid {} ({}, {pkg}) since {held_for}", h.pid, h.what),
                    None => format!("pid {} ({}) since {held_for}", h.pid, h.what),
                }
            }
            Entry::Unreadable { pid, why } => {
                format!("pid {pid}, whose record is unreadable ({why})")
            }
        })
        .collect();
    match described.len() {
        1 => format!("held by {}", described[0]),
        n => format!("held by {n} readers: {}", described.join("; ")),
    }
}

/// This process's command line, as the `what` a record carries.
///
/// argv is the most informative label available and the only one no caller can forget to pass —
/// `prova lock cargo -- cargo test` describes itself better than any hand-written role string.
/// argv[0] is reduced to its file name so the line stays readable, and the whole thing is bounded
/// so one long `prova eval` cannot turn a record into a wall of text.
pub fn describe_self() -> String {
    let mut parts: Vec<String> = std::env::args().collect();
    if let Some(first) = parts.first_mut() {
        if let Some(base) = Path::new(first.as_str()).file_name().and_then(|s| s.to_str()) {
            *first = base.to_string();
        }
    }
    truncate(&parts.join(" "), 200)
}

/// Bound a string to `max` characters, on a char boundary — a `&s[..max]` would panic on the
/// multibyte path a test name or an argument can easily contain.
fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some((cut, _)) => format!("{}…", &s[..cut]),
    }
}

/// A duration at the resolution an operator reads it in. Long waits are the whole subject here, so
/// "50925.4s" is the wrong answer even though it is the accurate one.
pub fn age(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 90.0 {
        format!("{secs:.1}s")
    } else if secs < 5400.0 {
        format!("{:.1}m", secs / 60.0)
    } else if secs < 172_800.0 {
        format!("{:.1}h", secs / 3600.0)
    } else {
        format!("{:.1}d", secs / 86_400.0)
    }
}

/// Is `pid` still running? `kill(pid, 0)` asks without signalling — the same check a stale-lock
/// reaper would use, except the kernel already reclaims flocks and only these sidecar files need
/// it. Shared with `barrier`, which needs exactly this question about its arrival files.
#[cfg(unix)]
pub fn process_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs error checking only; it never delivers anything to the process.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Windows has no `kill(pid, 0)`, and every caller of this only ever DELETES state on a `false`,
/// so the conservative answer is the safe one: assume alive and leave the file. The cost is a
/// stale record for a crashed holder; the alternative — assuming dead — would erase a live
/// holder's identity, which is the exact blindness this module exists to end.
#[cfg(not(unix))]
pub fn process_is_alive(_pid: i32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidecar rule, as a path assertion: records go BESIDE the lock, never into it. A change
    /// that made `dir_for` return the lock file itself would destroy mutual exclusion the first
    /// time a record was written over it, and nothing else in the suite would notice.
    #[test]
    fn records_live_beside_the_lock_file_never_in_it() {
        let lock = Path::new("/repo/.prova/var/locks/cargo.lock");
        let dir = dir_for(lock);
        assert_eq!(dir, Path::new("/repo/.prova/var/locks/cargo.holders"));
        assert_ne!(dir, lock, "the lock file's inode must never be written");
        assert_eq!(record_path(lock, 42), Path::new("/repo/.prova/var/locks/cargo.holders/42.json"));
    }

    /// Round-trip through the filesystem, plus the two states a reader must tell apart: a live
    /// record, and a live pid whose record is corrupt. Neither may read as "nobody is holding".
    #[test]
    fn a_registered_holder_is_readable_and_a_corrupt_one_is_reported_not_dropped() {
        let dir = crate::engine::make_tempdir().unwrap();
        let lock = dir.join("cargo.lock");
        let path = register(&lock, "cargo", false, Some(Path::new("/repo"))).unwrap();

        match read_all(&lock).as_slice() {
            [Entry::Live(h)] => {
                assert_eq!(h.pid, std::process::id());
                assert_eq!(h.token, "cargo");
                assert_eq!(h.package.as_deref(), Some("/repo"));
                assert!(!h.what.is_empty(), "a record names what is holding");
            }
            other => panic!("expected one live holder, got {other:?}"),
        }
        assert!(describe_holders(&lock).contains("pid "), "the narration names the pid");

        std::fs::write(&path, b"{ truncated").unwrap();
        match read_all(&lock).as_slice() {
            [Entry::Unreadable { pid, .. }] => assert_eq!(*pid, std::process::id()),
            other => panic!("a corrupt record must be reported, not dropped: {other:?}"),
        }

        unregister(&path);
        assert!(read_all(&lock).is_empty());
        assert!(
            describe_holders(&lock).contains("unregistered"),
            "no record is an unregistered holder, never an absent one"
        );
    }

    /// A record left by a process that is gone is swept on read. Without this, one crashed run
    /// makes every later `prova locks` accuse a pid that has not existed for days.
    #[cfg(unix)]
    #[test]
    fn a_dead_holders_record_is_swept_on_read() {
        let dir = crate::engine::make_tempdir().unwrap();
        let lock = dir.join("db.lock");
        std::fs::create_dir_all(dir_for(&lock)).unwrap();
        // A pid that cannot be alive, written by hand — registering always writes our own.
        let stale = dir_for(&lock).join("999999999.json");
        std::fs::write(&stale, b"{}").unwrap();
        assert!(read_all(&lock).is_empty(), "a dead holder is not a holder");
        assert!(!stale.exists(), "…and its record is collected");
    }

    /// Durations an operator reads. The incident that motivated this module ran 1 d 22 h and the
    /// wait behind it 50,925 s; rendering either in seconds is accurate and useless.
    #[test]
    fn ages_are_rendered_at_the_resolution_they_are_read_at() {
        assert_eq!(age(Duration::from_secs_f64(12.34)), "12.3s");
        assert_eq!(age(Duration::from_secs(372)), "6.2m");
        assert_eq!(age(Duration::from_secs(6840)), "1.9h");
        assert_eq!(age(Duration::from_secs_f64(50_925.4)), "14.1h", "the witnessed wait");
        assert_eq!(age(Duration::from_secs(166_500)), "46.2h", "the witnessed hold, 1 d 22 h");
        assert_eq!(age(Duration::from_secs(259_200)), "3.0d", "past two days, days read better");
    }

    /// Truncation on a char boundary. A slice at a byte index inside a multibyte character
    /// panics, and a command line is user-supplied text.
    #[test]
    fn a_long_label_is_bounded_without_splitting_a_character() {
        let wide = "é".repeat(300);
        let cut = truncate(&wide, 200);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().count(), 201);
        assert_eq!(truncate("short", 200), "short");
    }
}
