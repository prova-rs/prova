//! Package locks — the cross-instance readers-writer holds behind `locks = { … }`, as a
//! PUBLIC convention (docs/design/architecture.md#locks-cross-instance).
//!
//! A lock is a `flock(2)` on a well-known file: `<home>/.prova/var/locks/<token>.lock` for
//! package scope, `<temp>/prova-locks/<token>.lock` for machine scope. `LOCK_SH` is a reader,
//! `LOCK_EX` a writer, and the kernel releases a crashed holder instantly — no daemon, no
//! stale-lock reaping. **The file is the contract, not prova's process**: any external tool
//! (xtask, a Makefile, a CI step) joins a house rule like "one cargo at a time" by flocking
//! the same path. The scheduler takes these non-blocking (a refused leaf waits its turn); the
//! blocking [`hold_exclusive`] is for participants that ARE the critical section — a subject
//! provision, an external build wrapper.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Wall time this process spent STALLED on cross-instance locks
/// (docs/design/agent-ergonomics.md#narrate-lock-waits) — banked at the end of a run as
/// `run.lock_wait_ms`.
///
/// Stalled, not summed: the number answers "how much wall time would come back if the contention
/// vanished", so a wait overlapped with other work contributes nothing (it cost nothing) and two
/// leaves blocked on one token cannot double-count past the run's own clock. A metric that can
/// exceed wall time is worthless exactly when contention is worst.
///
/// Process-global because a prova process is one run, which keeps the counter out of every
/// signature between the flock and the record.
static STALLED_MS: AtomicU64 = AtomicU64::new(0);

/// Add to the run's stall total. Called by the blocking holds here and by the scheduler for each
/// poll round it spends with nothing runnable and a lock refused.
pub fn record_stall(waited: Duration) {
    STALLED_MS.fetch_add(waited.as_millis() as u64, Ordering::Relaxed);
}

/// The stall total so far, left in place — for a caller measuring one acquisition.
pub fn stalled() -> Duration {
    Duration::from_millis(STALLED_MS.load(Ordering::Relaxed))
}

/// The stall total, and reset to zero — what a run banks.
///
/// The reset is not tidiness: `prova mcp` serves MANY runs from one process (its warm loop takes
/// `Run` commands until the client goes away), so a counter that only accumulated would bank the
/// first run's contention again in the second, and again in the third. Per-run means take-and-clear.
pub fn take_stalled() -> Duration {
    Duration::from_millis(STALLED_MS.swap(0, Ordering::Relaxed))
}

/// Where `token`'s lock file lives. Run-scoped tokens (the reserved `__prova_` prefix) have
/// none. The token is sanitized into a filename; two tokens that sanitize identically share a
/// lock, which errs toward safety.
pub fn lock_path(token: &str, machine: bool, project_dir: Option<&Path>) -> Option<PathBuf> {
    if token.starts_with("__prova_") {
        return None;
    }
    let sanitized: String = token
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let dir = if machine {
        std::env::temp_dir().join("prova-locks")
    } else {
        match project_dir {
            Some(p) => p.join(".prova").join("var").join("locks"),
            // A bare run (no manifest) still honors the rule machine-wide rather than not at all.
            None => std::env::temp_dir().join("prova-locks"),
        }
    };
    Some(dir.join(format!("{sanitized}.lock")))
}

/// Open (creating as needed) and `flock` the token's file. Non-blocking: `Ok(None)` means
/// another holder has it — the scheduler's contract, where a refused leaf stays queued.
pub fn try_hold(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
) -> std::io::Result<Option<std::fs::File>> {
    let Some(path) = lock_path(token, machine, project_dir) else {
        return Ok(None);
    };
    let file = open_lock(&path)?;
    match flock(&file, shared, false)? {
        true => Ok(Some(file)),
        false => Ok(None),
    }
}

/// Block until `token`'s lock is held in the given mode, then return the holding handle —
/// dropping it releases. For participants that ARE the critical section (a subject provision,
/// an external build wrapper), where waiting is the point.
pub fn hold(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
) -> std::io::Result<std::fs::File> {
    let Some(path) = lock_path(token, machine, project_dir) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{token:?} is a reserved run-scoped token — it has no cross-instance lock"),
        ));
    };
    let file = open_lock(&path)?;
    // A blocking hold IS a stall: this thread is the critical section's participant, so nothing
    // else in the process proceeds while the flock waits. Measured here rather than at the call
    // sites so every caller — the subject provision, the `lock` wrapper — is counted once.
    let waited_from = Instant::now();
    flock(&file, shared, true)?;
    record_stall(waited_from.elapsed());
    Ok(file)
}

/// [`hold`], reporting how long the acquisition blocked — for a caller that narrates its own wait.
pub fn hold_timed(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
) -> std::io::Result<(std::fs::File, Duration)> {
    let before = stalled();
    let file = hold(token, shared, machine, project_dir)?;
    Ok((file, stalled().saturating_sub(before)))
}

/// [`hold`], write-mode and package-scoped — the common case, named for its callers.
pub fn hold_exclusive(
    token: &str,
    project_dir: Option<&Path>,
) -> std::io::Result<std::fs::File> {
    hold(token, false, false, project_dir)
}

fn open_lock(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path)
}

#[cfg(unix)]
fn flock(file: &std::fs::File, shared: bool, blocking: bool) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let mut op = if shared { libc::LOCK_SH } else { libc::LOCK_EX };
    if !blocking {
        op |= libc::LOCK_NB;
    }
    match unsafe { libc::flock(file.as_raw_fd(), op) } {
        0 => Ok(true),
        _ => {
            let err = std::io::Error::last_os_error();
            if !blocking && err.kind() == std::io::ErrorKind::WouldBlock {
                Ok(false)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(not(unix))]
fn flock(_file: &std::fs::File, _shared: bool, _blocking: bool) -> std::io::Result<bool> {
    // The Windows twin (LockFileEx) lands with the Windows runner; until then locks are
    // run-scoped there, which is exactly the pre-lock behavior.
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The convention's load-bearing facts: reserved tokens have no file, sanitization keeps a
    /// token addressable, and package/machine scopes key different directories.
    #[test]
    fn the_lock_file_is_a_predictable_contract() {
        assert_eq!(lock_path("__prova_serial__", false, None), None);
        let p = lock_path("cargo", false, Some(Path::new("/repo"))).unwrap();
        assert_eq!(p, Path::new("/repo/.prova/var/locks/cargo.lock"));
        let weird = lock_path("a b/c", false, Some(Path::new("/repo"))).unwrap();
        assert_eq!(weird.file_name().unwrap(), "a-b-c.lock");
        let machine = lock_path("cargo", true, Some(Path::new("/repo"))).unwrap();
        assert!(machine.starts_with(std::env::temp_dir()), "machine scope leaves the package");
    }

    /// Exclusive excludes shared and vice versa, across separate descriptors — the semantics
    /// every participant (scheduler, provision, external tool) relies on.
    #[test]
    fn holds_exclude_across_descriptors() {
        let dir = crate::engine::make_tempdir().unwrap();
        let writer = try_hold("t", false, false, Some(&dir)).unwrap();
        assert!(writer.is_some());
        assert!(try_hold("t", true, false, Some(&dir)).unwrap().is_none(), "reader waits on writer");
        drop(writer);
        let r1 = try_hold("t", true, false, Some(&dir)).unwrap();
        let r2 = try_hold("t", true, false, Some(&dir)).unwrap();
        assert!(r1.is_some() && r2.is_some(), "readers share");
        assert!(try_hold("t", false, false, Some(&dir)).unwrap().is_none(), "writer waits on readers");
    }
}

#[cfg(test)]
mod stall_tests {
    use super::*;

    // The MCP shape: one process, many runs. A second run must bank its OWN contention, not the
    // first run's again — the reason `take_stalled` clears rather than reads.
    #[test]
    fn taking_the_stall_total_clears_it_for_the_next_run() {
        take_stalled(); // other tests share the process; start from a known floor
        record_stall(Duration::from_millis(120));
        assert_eq!(take_stalled(), Duration::from_millis(120));
        assert_eq!(take_stalled(), Duration::ZERO);
        record_stall(Duration::from_millis(5));
        assert_eq!(take_stalled(), Duration::from_millis(5));
    }
}
