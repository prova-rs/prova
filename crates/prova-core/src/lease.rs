//! The conduct lease (docs/design/verifiers.md#conduct-lease-survives-prova-death).
//!
//! A conduct's right to run is leased from the run, and the lease is enforced by something that
//! survives prova's worst death: cleanup code cannot be the mechanism, because the deaths that
//! matter most (SIGKILL, OOM, a panic, CI's SIGTERM) run no destructors. On unix the holder is a
//! **reaper sidecar** — `prova reap`, the same static binary — spawned lazily with the first
//! conduct: it reads `+<pgid>` / `-<pgid>` registrations on its stdin, and the kernel closes that
//! pipe when prova dies *however it dies*; EOF is the trigger to sweep every still-registered
//! process group and exit. While prova lives, controlled kills (`killpg`) deregister on the way
//! out, so the reaper's steady state is an empty set and a clean exit.
//!
//! The reaper spawns into its own process group so the terminal's Ctrl-C cannot kill the janitor
//! before it sweeps — which is how interrupt behavior gets STRONGER than the shared-group
//! accident it replaces. `prova start` opts the whole invocation out
//! (docs/design/verifiers.md#detached-topologies-hold-no-lease): outliving the invocation is that
//! verb's purpose. Windows has no reaper yet — job objects are the windows lane's business — so
//! leases are a unix fact and Windows keeps direct-child kills, stated in the claim.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Detached provisioning (`prova start`) calls this once, before any factory runs: nothing this
/// invocation spawns takes a lease, because surviving the invocation is the point.
pub fn set_detached() {
    ENABLED.store(false, Ordering::Relaxed);
}

#[cfg(unix)]
mod imp {
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};

    /// The reaper's stdin, if the sidecar could be spawned. `None` (spawn failed, no exe path)
    /// degrades to no lease — today's behavior, never an error a conduct pays for.
    static REAPER: OnceLock<Option<Mutex<std::process::ChildStdin>>> = OnceLock::new();

    fn spawn_reaper() -> Option<Mutex<std::process::ChildStdin>> {
        use std::os::unix::process::CommandExt;
        let exe = crate::current_exe().ok()?;
        let mut child = std::process::Command::new(exe)
            .arg("reap")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        // Deliberately never waited on: it must outlive every conduct and exits on OUR death.
        // While we live it runs; when we die it sweeps, exits, and reparents — no zombie of ours.
        std::mem::forget(child);
        Some(Mutex::new(stdin))
    }

    fn reaper() -> Option<&'static Mutex<std::process::ChildStdin>> {
        REAPER.get_or_init(spawn_reaper).as_ref()
    }

    pub(super) fn send(line: &str) -> bool {
        let Some(tx) = reaper() else { return false };
        let mut tx = tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        writeln!(tx, "{line}").and_then(|()| tx.flush()).is_ok()
    }
}

/// RAII lease for one conduct's process group. Registered at spawn, deregistered on drop (the
/// controlled paths: completion, or a kill this process performed itself). If prova dies while
/// the lease is live, no drop runs — which is exactly when the reaper's registration matters.
pub struct Lease {
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
}

impl Lease {
    /// Lease the group led by `pid` (a child spawned with `process_group(0)`, so its pid IS its
    /// pgid). No-ops — returning an inert guard — when detached, on non-unix, or if the reaper
    /// could not spawn.
    pub fn register(pid: Option<u32>) -> Lease {
        #[cfg(unix)]
        if ENABLED.load(Ordering::Relaxed) {
            if let Some(p) = pid {
                if imp::send(&format!("+{p}")) {
                    return Lease { pid: Some(p) };
                }
            }
        }
        let _ = pid;
        Lease { pid: None }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(p) = self.pid {
            imp::send(&format!("-{p}"));
        }
    }
}

/// Kill the whole process group led by `pid` (spawned with `process_group(0)`), then let the
/// caller reap the direct child. Non-unix: the direct-child kill the caller already performs is
/// all there is.
pub(crate) fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(p) = pid {
        // SAFETY: killpg with SIGKILL on a group we created; an already-gone group is ESRCH,
        // which is fine — the caller's child-kill reaps whatever remains.
        unsafe {
            libc::killpg(p as i32, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// The sidecar's own loop (`prova reap`): read registrations until stdin closes — which the
/// kernel does when the spawning prova dies, however it dies — then sweep what is still leased.
pub fn reaper_main() -> i32 {
    #[cfg(unix)]
    {
        use std::io::BufRead;
        let mut live: std::collections::HashSet<i32> = std::collections::HashSet::new();
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if let Some(p) = line.strip_prefix('+') {
                if let Ok(p) = p.parse::<i32>() {
                    live.insert(p);
                }
            } else if let Some(p) = line.strip_prefix('-') {
                if let Ok(p) = p.parse::<i32>() {
                    live.remove(&p);
                }
            }
        }
        for pgid in live {
            // SAFETY: sweeping groups the (now dead) holder leased; ESRCH for the already-gone.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
    0
}
