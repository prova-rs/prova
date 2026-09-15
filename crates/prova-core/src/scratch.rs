//! Where prova puts machine-scoped transient state — and who reaps it.
//!
//! Three backlog items arrived separately and were one defect
//! (`architecture.md#machine-lock-dir-follows-tmpdir`,
//! `placement.md#broker-leaves-its-workspace-root`,
//! `agent-ergonomics.md#scope-tempdirs-outlive-a-run-that-never-tears-down`): `std::env::temp_dir()`
//! was the address for state with two incompatible requirements.
//!
//! **A contract address must be stable.** The machine-scoped lock directory has to resolve to the
//! same path in every process on the host, or "one cargo at a time, machine-wide" quietly becomes
//! two. `temp_dir()` reads `$TMPDIR`, and `$TMPDIR` is forkable by anything that sets it for its
//! children — a cargo `[env]` table, a CI wrapper, a sandboxed app. Measured 2026-09-14: a
//! containment that set `TMPDIR=~/.cache/test-tmp` for cargo-run processes split the machine lock
//! in half, silently.
//!
//! **A scratch root must be reapable.** Scope directories and broker workspaces are removed on a
//! teardown path, which is exactly the path a killed or crashed run does not take. Measured the
//! same day: 6,683 scope directories from 1,543 dead pids, and 220 broker roots whose owners were
//! all gone.
//!
//! Both needs are served by one base plus a naming convention: a root is named for the process
//! that owns it, so anyone can tell a live owner's scratch from a dead one's without a registry.

use std::path::{Path, PathBuf};

/// Override for the base, for a caller that must place it explicitly (a sandbox, a test, a CI
/// image with a tmpfs it wants used). The one escape hatch, named so it cannot be set by accident.
pub const BASE_ENV: &str = "PROVA_SCRATCH_DIR";

/// The machine-wide base for prova's transient state. **Never `$TMPDIR`.**
///
/// Resolution, in order, each step chosen because the environment cannot fork it:
///
/// 1. `PROVA_SCRATCH_DIR` — explicit, deliberate, and the only way to move it.
/// 2. **macOS**: `confstr(_CS_DARWIN_USER_TEMP_DIR)`, the per-user temp dir the kernel reports.
///    This is what `getconf DARWIN_USER_TEMP_DIR` prints, and it is independent of `$TMPDIR` —
///    verified by forking `TMPDIR` and watching it not move.
/// 3. `$XDG_RUNTIME_DIR` — the same idea on Linux: per-user, per-boot, set by the session manager
///    rather than inherited from whoever spawned you.
/// 4. A fixed per-user path under the home directory. Not `temp_dir()`: falling back to the
///    forkable thing would re-introduce the split on exactly the machines that have neither of the
///    above, which is where nobody would think to look for it.
pub fn base() -> PathBuf {
    if let Some(dir) = std::env::var_os(BASE_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("prova");
    }
    #[cfg(target_os = "macos")]
    if let Some(dir) = darwin_user_temp_dir() {
        return dir.join("prova");
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("prova");
    }
    fixed_per_user_base()
}

/// The last resort, named so it can be tested.
///
/// Stable per-user, though not per-boot: a path that survives a reboot is a cosmetic flaw, while a
/// path that differs between two processes on one machine is the bug this module exists for. It is
/// reached only where neither the Darwin per-user temp dir nor `XDG_RUNTIME_DIR` exists, which is
/// why no black-box proof can reach it on macOS — mutation testing said so, and this function
/// exists so a unit test can.
fn fixed_per_user_base() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("prova")
        .join("scratch")
}


/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` — the per-user temp dir, straight from the kernel.
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    // SAFETY: confstr writes at most `len` bytes into the buffer and reports the size it needed;
    // a zero return means the name is unsupported, which is handled as "no answer".
    unsafe {
        let needed = libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0);
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed];
        let wrote = libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        );
        if wrote == 0 {
            return None;
        }
        // confstr's count INCLUDES the trailing NUL; the path is everything before it.
        let bytes = &buf[..wrote.saturating_sub(1)];
        std::str::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// The machine-scoped lock directory — the contract's address
/// (`architecture.md#machine-lock-dir-follows-tmpdir`).
pub fn locks_dir() -> PathBuf {
    base().join("locks")
}

/// A root owned by THIS process, for scratch of the given kind (`run`, `broker`).
///
/// The pid is in the name rather than in a registry because the whole point is that it survives
/// its owner: a sweeper arriving hours later needs to decide "is this reapable" from the name
/// alone, with no file to have failed to write.
pub fn owned_root(kind: &str) -> PathBuf {
    base().join(format!("{kind}-{}", std::process::id()))
}

/// Remove every `<kind>-<pid>` root under the base whose pid is no longer alive.
///
/// Best-effort by construction: a root being removed by its own owner concurrently, a permission
/// error, a pid that was recycled — none of those are worth failing a run over, and all of them
/// resolve on the next sweep. Returns how many roots were removed, for the caller that wants to
/// say so.
pub fn sweep_dead(kind: &str) -> usize {
    let prefix = format!("{kind}-");
    let Ok(entries) = std::fs::read_dir(base()) else { return 0 };
    let mut swept = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = name.strip_prefix(&prefix).and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if pid == std::process::id() || is_alive(pid) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            swept += 1;
        }
    }
    swept
}

/// Is `pid` still running? A dead owner is the only thing that makes its root reapable.
///
/// `kill(pid, 0)` asks the kernel without sending anything. `EPERM` means the process exists and
/// belongs to someone else — alive, and emphatically not ours to reap — so it is treated as live.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 performs error checking only; it never delivers a signal.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true // No cheap probe here; never reap what we cannot confirm is dead.
    }
}

/// The startup sweep: reap every dead owner's root, once per process.
///
/// Called unconditionally and early, for every verb — one `read_dir` plus a `kill(0)` per root is
/// nothing, and making it unconditional is what makes every prova invocation self-healing rather
/// than only the ones that happen to allocate scratch.
///
/// **Only ever inside our own base.** Directories left by older provas live in `$TMPDIR` under a
/// different naming scheme, and sweeping those would mean deleting paths outside the tree this
/// module controls, matched by a pattern that could plausibly hit something else. A stale
/// directory is untidy; deleting a stranger's files is the kind of housekeeping that ruins an
/// afternoon. Those are reported by `prova locks`-style tooling and removed by hand, once.
pub fn boot() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| {
        for kind in ["run", "broker"] {
            sweep_dead(kind);
        }
    });
}

/// Create `dir` and every parent, returning it — the one-liner every caller above needs.
pub fn ensure(dir: PathBuf) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Remove a root this process owns, ignoring an already-gone one.
pub fn remove(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback must not be `$TMPDIR`-derived — the entire point of the resolution order.
    /// Unreachable on macOS (the Darwin branch answers first), so it is asserted directly.
    #[test]
    fn the_last_resort_is_not_tmpdir_derived() {
        let fallback = fixed_per_user_base();
        let tmp = std::env::temp_dir();
        assert!(
            !fallback.starts_with(&tmp),
            "the last resort {fallback:?} sits under $TMPDIR {tmp:?} — it would fork with it"
        );
        assert!(fallback.ends_with("scratch"), "got {fallback:?}");
    }

    /// `EPERM` means the process EXISTS and belongs to someone else. Treating that as dead would
    /// reap another user's scratch. pid 1 is owned by root and always running, so a non-root test
    /// process gets exactly that error — the only way to reach this branch without a second user.
    #[test]
    #[cfg(unix)]
    fn another_users_live_process_counts_as_alive() {
        // Skip the assertion when running AS root, where kill(1, 0) succeeds outright and the
        // EPERM branch is genuinely not reachable.
        let as_root = unsafe { libc::geteuid() } == 0;
        if as_root {
            return;
        }
        assert!(is_alive(1), "pid 1 exists and is not ours — EPERM must read as alive");
    }

    /// A pid that cannot exist is dead, or nothing would ever be reaped.
    #[test]
    #[cfg(unix)]
    fn an_impossible_pid_is_dead() {
        assert!(!is_alive(0x7FFF_FFFE), "an unused pid must read as dead");
    }
}
