//! `prova.barrier(token, parties, opts?)` — proving two things were in flight AT ONCE.
//!
//! **Why this is a primitive and not a recipe.** A proof that asserts concurrency had exactly three
//! tools before this: `prova.sleep`, `prova.retry`, and the locks — and the locks SERIALIZE, which
//! is the opposite. So "my service handles two concurrent requests" was written by starting both,
//! sleeping, and comparing timestamps. That assertion is a vacuous-proof generator in both
//! directions: it fails when a loaded host simply did not schedule the second unit inside the
//! sleep window (measured twice in one night on prova's own suite), and it PASSES when the sleeps
//! are long enough to overlap even though the system serialized. Either way what is being measured
//! is timing luck, not the property.
//!
//! **A barrier turns the observation into a precondition.** Reaching the far side is itself proof
//! that `parties` participants were simultaneously inside it — there is no window to get lucky
//! with, and nothing to compare afterwards. That is the same move as making a decoded array carry
//! its own array-ness: the wrong state stops being representable rather than being detected.
//!
//! **The substrate is the one locks already use** — a file under `.prova/var/`, so participants
//! find each other across Lua states, across workers, and across prova instances, with the kernel
//! cleaning up after a crashed holder. No daemon, no shared mutable Lua state (which `Scope.Run`
//! deliberately does not offer: its values are per-state data copies).
//!
//! **A barrier makes its participants one atomic selection unit.** Selecting some of them — `-k`,
//! `--node`, `--last-failed` — leaves the rest waiting for parties that were never going to run.
//! That is not a defect to design away (the runtime cannot know who will call a barrier until they
//! do), but it IS the most likely cause in practice, so the timeout says so first. It is also the
//! reason to reach for a barrier only when concurrency is the property under proof: it couples
//! units that `prova.group` otherwise keeps independent by design.
//!
//! **Failure is loud and names the shortfall.** A barrier that times out reports how many arrived
//! of how many expected, because the two reasons it happens want different fixes: the system under
//! test serialized when it should not have, or the suite cannot run them concurrently at all
//! (`-j 1`, or both units holding one exclusive lock). A hang would report neither.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a participant waits for the others before calling it serialized.
///
/// A default rather than a required argument, matching `prova.retry` and `http.wait_for`: the
/// number is a patience, and every caller having to invent one adds ceremony without adding
/// judgement. What matters for safety is that it is BOUNDED — there is no spelling of `barrier`
/// that waits forever, so a missing participant costs a bounded failure rather than a hung run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Reclaim barrier files left by processes that are gone.
///
/// Cleanup is a separate concern from release, deliberately. A participant that TIMES OUT must not
/// remove the file: a sibling mid-poll would see it missing, and "missing" is how a completed
/// barrier reports itself — so giving up would RELEASE the others, turning a failed rendezvous
/// into a passing one. Only the last arrival unlinks.
///
/// That leaves two kinds of litter, and both are collected here rather than at the failure site: a
/// timed-out barrier's partial count, and a crashed run's file. Both are keyed to a pid, so a file
/// whose process no longer exists can never be part of a live rendezvous and is safe to remove.
/// Runs once per process, on the first barrier — no engine hook to forget to call.
fn sweep_orphans(dir: &std::path::Path) {
    static SWEPT: OnceLock<()> = OnceLock::new();
    if SWEPT.set(()).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(pid) = name
            .strip_suffix(".barrier")
            .and_then(|stem| stem.rsplit_once('-'))
            .and_then(|(_, pid)| pid.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() as i32 || process_is_alive(pid) {
            continue;
        }
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Is `pid` still running? `kill(pid, 0)` asks without signalling — the same check a stale-lock
/// reaper would use, except the kernel already reclaims flocks and only these count files need it.
fn process_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs error checking only; it never delivers anything to the process.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Where a barrier's arrival file lives. Beside the locks, and sanitized the same way — two tokens
/// that sanitize identically share a barrier, which errs toward blocking rather than toward two
/// groups silently passing through each other.
///
/// **The process id is in the name, and this is load-bearing.** Arrival state that outlived its
/// run made the barrier pass VACUOUSLY: a first run left the count at `parties`, and every later
/// run saw a satisfied barrier and sailed through without anything overlapping — precisely the
/// timing-luck pass this primitive exists to make impossible. Participants are workers of one
/// process, so keying on the pid means a new run cannot inherit an old run's arrivals. Completion
/// unlinks the file as well ([`arrive`]), so a barrier reused within one process starts clean too.
pub fn barrier_path(token: &str, project_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    let sanitized: String = token
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let dir = project_dir?.join(".prova/var/barriers");
    Some(dir.join(format!("{sanitized}-{}.barrier", std::process::id())))
}

/// What a completed wait reports back.
#[derive(Debug)]
pub struct Arrival {
    /// This participant's 1-based arrival order — so a proof can assert on WHO got there first
    /// when that matters, and so the first arrival can do one-time setup.
    pub position: u64,
    /// How long this participant waited for the rest.
    pub waited: Duration,
}

/// Register this participant's arrival and hand back the path to poll and its position.
///
/// Separate from the waiting so the CALLER chooses how to idle. That is not a style preference:
/// prova's per-worker concurrency is COOPERATIVE — many Lua coroutines on one current-thread
/// runtime — so a barrier that blocks the thread starves the very sibling it is waiting for and
/// deadlocks itself. The Lua binding awaits; a Rust thread blocks. Both drive the same state.
pub fn join(
    token: &str,
    parties: u64,
    project_dir: Option<&std::path::Path>,
) -> Result<(PathBuf, u64), String> {
    if parties == 0 {
        return Err("barrier: `parties` must be at least 1".to_string());
    }
    let path = barrier_path(token, project_dir)
        .ok_or_else(|| "barrier: no package directory to place the barrier in".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("barrier {token:?}: creating {}: {e}", parent.display()))?;
        sweep_orphans(parent);
    }
    let position = bump(&path, token)?;
    Ok((path, position))
}

/// Has everyone arrived? A missing file means the last party passed through and cleaned up —
/// `join` created it, so it can only be gone because the barrier completed.
pub fn released(path: &std::path::Path, token: &str, parties: u64) -> Result<bool, String> {
    match read_count(path, token) {
        Ok(n) => Ok(n >= parties),
        Err(_) if !path.exists() => Ok(true),
        Err(e) => Err(e),
    }
}

/// The last party's cleanup: leave nothing that could satisfy the next barrier.
pub fn release(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// How many have arrived, for a timeout message that names the shortfall.
pub fn arrived(path: &std::path::Path, token: &str) -> u64 {
    read_count(path, token).unwrap_or(0)
}

/// The message a timed-out participant reports. The two reasons want different fixes, so both are
/// named: the system serialized when it should not have, or this suite cannot run them at once.
pub fn timeout_message(token: &str, parties: u64, arrived: u64, waited: Duration) -> String {
    format!(
        "barrier {token:?}: waited {waited:?} for {parties} participants, {arrived} arrived. \
         Three reasons, and they want different fixes: (1) the other participants were not \
         SELECTED — a barrier makes its participants one atomic unit, so `-k`, `--node` or \
         `--last-failed` that picks some of them leaves the rest waiting alone, and nothing is \
         wrong; (2) this suite cannot run them at once (`-j 1`, a group with `parallel = false`, \
         or both units holding one exclusive lock); (3) the units genuinely did not run \
         concurrently — which IS the finding, if concurrency is what you meant to assert."
    )
}

/// Arrive at `token` and BLOCK until `parties` participants have — for callers on their own
/// thread. Async callers use [`join`] + [`released`] so they yield instead.
pub fn arrive(
    token: &str,
    parties: u64,
    timeout: Duration,
    project_dir: Option<&std::path::Path>,
) -> Result<Arrival, String> {
    let started = Instant::now();
    let (path, position) = join(token, parties, project_dir)?;

    if position >= parties {
        release(&path);
        return Ok(Arrival { position, waited: started.elapsed() });
    }

    loop {
        if released(&path, token, parties)? {
            return Ok(Arrival { position, waited: started.elapsed() });
        }
        if started.elapsed() >= timeout {
            return Err(timeout_message(token, parties, arrived(&path, token), started.elapsed()));
        }
        // Short enough that a barrier costs little once the last party lands, long enough not to
        // spin a core while waiting on a slow neighbour.
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Add one to the arrival count, under an exclusive hold, and return this participant's position.
fn bump(path: &std::path::Path, token: &str) -> Result<u64, String> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("barrier {token:?}: opening {}: {e}", path.display()))?;
    let _hold = ExclusiveHold::take(&file)
        .map_err(|e| format!("barrier {token:?}: locking {}: {e}", path.display()))?;

    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| format!("barrier {token:?}: reading: {e}"))?;
    let next = text.trim().parse::<u64>().unwrap_or(0) + 1;

    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.set_len(0))
        .and_then(|_| file.write_all(next.to_string().as_bytes()))
        .and_then(|_| file.flush())
        .map_err(|e| format!("barrier {token:?}: writing: {e}"))?;
    Ok(next)
}

/// The current arrival count, under a shared hold so a partial write is never observed.
fn read_count(path: &std::path::Path, token: &str) -> Result<u64, String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("barrier {token:?}: reading {}: {e}", path.display()))?;
    let _hold = SharedHold::take(&file)
        .map_err(|e| format!("barrier {token:?}: locking {}: {e}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| format!("barrier {token:?}: reading: {e}"))?;
    Ok(text.trim().parse::<u64>().unwrap_or(0))
}

macro_rules! hold {
    ($name:ident, $op:expr) => {
        /// An flock held for the life of the value — released by the kernel even if we panic.
        struct $name(std::os::fd::RawFd);
        impl $name {
            fn take(file: &std::fs::File) -> std::io::Result<Self> {
                use std::os::fd::AsRawFd;
                let fd = file.as_raw_fd();
                // SAFETY: `fd` is owned by `file`, which outlives this hold at every call site.
                if unsafe { libc::flock(fd, $op) } != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(Self(fd))
            }
        }
        impl Drop for $name {
            fn drop(&mut self) {
                // SAFETY: same fd, still open — the file outlives the hold.
                unsafe { libc::flock(self.0, libc::LOCK_UN) };
            }
        }
    };
}
hold!(ExclusiveHold, libc::LOCK_EX);
hold!(SharedHold, libc::LOCK_SH);

#[cfg(test)]
mod tests {
    use super::*;

    /// A home no other test shares.
    ///
    /// Keyed per CALL, not per process. Keyed on the pid alone, every test in this module got the
    /// same directory — and since each one wipes it on the way in, one test deleted another's
    /// barrier state mid-run. Under `cargo test` these are threads in a single process, so that is
    /// a live race; under nextest they are separate processes, so it is invisible. `prova run ut`
    /// deputes to nextest, which is exactly why this passed locally for a day and then failed the
    /// release gate, whose Setup leg runs plain `cargo test`.
    ///
    /// A barrier is shared state by construction — the whole primitive is "have N parties met
    /// here" — so its tests are the ones that can least afford to share a home by accident.
    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "prova-barrier-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// One party is already everyone: the barrier must not block waiting for a second.
    #[test]
    fn a_single_party_passes_immediately() {
        let dir = tmp();
        let a = arrive("solo", 1, Duration::from_secs(5), Some(&dir)).unwrap();
        assert_eq!(a.position, 1);
        assert!(a.waited < Duration::from_secs(1));
    }

    /// The failure that matters: nobody else came. The message has to distinguish the two reasons
    /// — the SUT serialized, or the suite cannot run them at once — because they want different
    /// fixes and a bare timeout points at neither.
    #[test]
    fn a_lone_participant_times_out_and_says_how_many_arrived() {
        let dir = tmp();
        let err = arrive("lonely", 2, Duration::from_millis(120), Some(&dir)).unwrap_err();
        assert!(err.contains("1 arrived"), "names the shortfall: {err}");
        assert!(err.contains("did not run concurrently"), "names the finding: {err}");
        assert!(err.contains("-j 1"), "…and the other explanation: {err}");
    }

    /// Two real threads, which is the case the primitive exists for: neither can pass until both
    /// are inside, so reaching the far side IS the proof they overlapped.
    #[test]
    fn two_participants_release_each_other() {
        let dir = tmp();
        let d2 = dir.clone();
        let other = std::thread::spawn(move || {
            arrive("pair", 2, Duration::from_secs(10), Some(&d2)).map(|a| a.position)
        });
        let mine = arrive("pair", 2, Duration::from_secs(10), Some(&dir)).unwrap();
        let theirs = other.join().unwrap().unwrap();
        let mut seen = [mine.position, theirs];
        seen.sort_unstable();
        assert_eq!(seen, [1, 2], "each got a distinct position under the exclusive hold");
    }

    /// The bug this primitive nearly shipped WITH: arrival state that outlives its run makes every
    /// later barrier pass without anything overlapping — a vacuous pass, from the very thing built
    /// to make vacuous passes impossible. Completion must leave nothing behind.
    #[test]
    fn a_completed_barrier_leaves_no_state_to_satisfy_the_next_one() {
        let dir = tmp();
        let path = barrier_path("reused", Some(&dir)).unwrap();

        let d2 = dir.clone();
        let other = std::thread::spawn(move || arrive("reused", 2, Duration::from_secs(10), Some(&d2)));
        arrive("reused", 2, Duration::from_secs(10), Some(&dir)).unwrap();
        other.join().unwrap().unwrap();
        assert!(!path.exists(), "completion cleans up: {}", path.display());

        // So a second use of the same token starts from zero rather than inheriting the first's
        // arrivals — which, before the fix, let a lone participant sail straight through.
        let err = arrive("reused", 2, Duration::from_millis(120), Some(&dir)).unwrap_err();
        assert!(err.contains("1 arrived"), "the next barrier really counts again: {err}");
    }

    #[test]
    fn zero_parties_is_refused_rather_than_hanging() {
        let dir = tmp();
        assert!(arrive("nobody", 0, Duration::from_millis(50), Some(&dir))
            .unwrap_err()
            .contains("at least 1"));
    }
}
