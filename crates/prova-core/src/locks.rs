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

/// Block until `token`'s exclusive lock is held, then return the holding handle — dropping it
/// releases. For participants that ARE the critical section (a subject provision, an external
/// build wrapper), where waiting is the point.
pub fn hold_exclusive(
    token: &str,
    project_dir: Option<&Path>,
) -> std::io::Result<std::fs::File> {
    let Some(path) = lock_path(token, false, project_dir) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{token:?} is a reserved run-scoped token — it has no cross-instance lock"),
        ));
    };
    let file = open_lock(&path)?;
    flock(&file, false, true)?;
    Ok(file)
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
