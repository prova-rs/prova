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
//!
//! **A hold is a promise to everyone else on the box, so it is nameable and its wait is
//! bounded** (docs/plans/lock-starvation.md). Two things beyond the flock itself:
//! [`holder`] writes a sidecar record naming the pid, package and command behind each hold, and
//! the blocking waits below narrate what they are waiting for while they wait, on an escalating
//! cadence, with an optional bound. The kernel releasing a DEAD holder was long treated as
//! sufficient safety; it is not, because a hung holder is alive (2026-09-01: 1 d 22 h on one
//! `cargo` token, and a 14 h wait behind it that could name nobody).

pub mod holder;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
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

/// A held lock: the flock handle, plus the holder record to remove when it goes.
///
/// Dropping releases, exactly as the bare `File` it replaces did. What the wrapper adds is that
/// the sidecar record cannot outlive the hold by being forgotten at one of the several release
/// paths — there is only one, and it is `Drop`.
#[derive(Debug)]
pub struct Hold {
    /// Dropping this releases the flock. Declared second so it drops after [`Hold::drop`] runs.
    record: Option<PathBuf>,
    file: std::fs::File,
}

impl Hold {
    /// The underlying handle, for a caller that needs the descriptor itself.
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        // The record goes FIRST, and the two windows this orders are not symmetric. A record that
        // outlived its flock would be a phantom holder — naming a pid that excludes nobody, which
        // is the fail-open direction for anyone diagnosing contention. A flock that briefly
        // outlives its record reads as an unregistered holder, a state the convention already has
        // a truthful word for.
        if let Some(path) = self.record.take() {
            holder::unregister(&path);
        }
    }
}

/// How a blocking hold behaves while it waits.
///
/// The default is `narrate_every: 60s` and **no bound**: this change makes a wait visible without
/// changing when any work dies (docs/plans/lock-starvation.md, sequencing). `PROVA_LOCK_WAIT_TIMEOUT`
/// opts into a bound today; the defaults decision rides with the hold-side supervision that makes
/// a bound rarely necessary in the first place.
#[derive(Debug, Clone)]
pub struct WaitPolicy {
    /// Give up after this long. `None` waits forever.
    pub bound: Option<Duration>,
    /// Re-narrate the wait this often. `None` narrates nothing.
    pub narrate_every: Option<Duration>,
}

impl Default for WaitPolicy {
    fn default() -> Self {
        WaitPolicy { bound: bound_from_env(), narrate_every: narrate_every_from_env() }
    }
}

/// `PROVA_LOCK_NARRATE_EVERY` — how often a long wait re-says what it is waiting for. 60s by
/// default; `0`/`off` silences the repeats (the first line at the call site still fires). A CI job
/// that logs to a file wants it wider, a human watching a terminal wants it narrower, and the
/// suite needs it narrow enough to prove the repeat happens at all.
fn narrate_every_from_env() -> Option<Duration> {
    match std::env::var("PROVA_LOCK_NARRATE_EVERY") {
        Err(_) => Some(Duration::from_secs(60)),
        Ok(raw) => match parse_bound(&raw) {
            Ok(every) => every,
            Err(why) => {
                static WARNED: AtomicBool = AtomicBool::new(false);
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!("prova: PROVA_LOCK_NARRATE_EVERY: {why} — narrating every 60s");
                }
                Some(Duration::from_secs(60))
            }
        },
    }
}

/// A wait bound as an operator spells it: a humantime duration (`30m`, `2h`), or `0`/`off`/`never`
/// to say "wait forever" deliberately. Shared by the env var and `prova lock --wait-timeout` so
/// the two spellings cannot drift into accepting different things.
pub fn parse_bound(raw: &str) -> Result<Option<Duration>, String> {
    let raw = raw.trim();
    if raw.is_empty() || matches!(raw, "0" | "off" | "never") {
        return Ok(None);
    }
    humantime::parse_duration(raw)
        .map(Some)
        .map_err(|e| format!("{raw:?} is not a duration ({e}) — use 30m, 2h, or 0 for 'forever'"))
}

/// `PROVA_LOCK_WAIT_TIMEOUT`. An unparseable value warns ONCE and waits forever: an env var typo
/// must not be the reason a build dies, and it must not be silent either.
fn bound_from_env() -> Option<Duration> {
    let raw = std::env::var("PROVA_LOCK_WAIT_TIMEOUT").ok()?;
    match parse_bound(&raw) {
        Ok(bound) => bound,
        Err(why) => {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!("prova: PROVA_LOCK_WAIT_TIMEOUT: {why} — waiting without a bound");
            }
            None
        }
    }
}

/// The directory holding `machine`-scoped or package-scoped lock files. Public because it IS the
/// contract's address: "join the house rule by flocking a file in here" is the whole convention,
/// and a tool that has to guess the path cannot join it.
pub fn lock_dir(machine: bool, project_dir: Option<&Path>) -> PathBuf {
    match (machine, project_dir) {
        (false, Some(p)) => p.join(".prova").join("var").join("locks"),
        // A bare run (no manifest) still honors the rule machine-wide rather than not at all.
        _ => std::env::temp_dir().join("prova-locks"),
    }
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
    Some(lock_dir(machine, project_dir).join(format!("{sanitized}.lock")))
}

/// Open (creating as needed) and `flock` the token's file. Non-blocking: `Ok(None)` means
/// another holder has it — the scheduler's contract, where a refused leaf stays queued.
pub fn try_hold(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
) -> std::io::Result<Option<Hold>> {
    let Some(path) = lock_path(token, machine, project_dir) else {
        return Ok(None);
    };
    let file = open_lock(&path)?;
    match flock(&file, shared, false)? {
        true => Ok(Some(registered(file, &path, token, shared, project_dir))),
        false => Ok(None),
    }
}

/// Block until `token`'s lock is held in the given mode, then return the holding handle —
/// dropping it releases. For participants that ARE the critical section (a subject provision,
/// an external build wrapper), where waiting is the point.
///
/// The wait narrates itself on the default [`WaitPolicy`]; [`hold_with`] takes an explicit one.
pub fn hold(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
) -> std::io::Result<Hold> {
    hold_with(token, shared, machine, project_dir, &WaitPolicy::default())
}

/// [`hold`] under an explicit wait policy.
pub fn hold_with(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
    policy: &WaitPolicy,
) -> std::io::Result<Hold> {
    let Some(path) = lock_path(token, machine, project_dir) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{token:?} is a reserved run-scoped token — it has no cross-instance lock"),
        ));
    };
    let file = open_lock(&path)?;
    // The uncontended path must not pay for a thread — it is every hold in a quiet tree.
    if flock(&file, shared, false)? {
        return Ok(registered(file, &path, token, shared, project_dir));
    }
    // A blocking hold IS a stall: this thread is the critical section's participant, so nothing
    // else in the process proceeds while the flock waits. Measured here rather than at the call
    // sites so every caller — the subject provision, the `lock` wrapper — is counted once.
    let waited_from = Instant::now();
    let file = wait_for_flock(file, &path, token, shared, policy)?;
    record_stall(waited_from.elapsed());
    Ok(registered(file, &path, token, shared, project_dir))
}

/// [`hold`], reporting how long the acquisition blocked — for a caller that narrates its own wait.
pub fn hold_timed(
    token: &str,
    shared: bool,
    machine: bool,
    project_dir: Option<&Path>,
) -> std::io::Result<(Hold, Duration)> {
    let before = stalled();
    let file = hold(token, shared, machine, project_dir)?;
    Ok((file, stalled().saturating_sub(before)))
}

/// [`hold`], write-mode and package-scoped — the common case, named for its callers.
pub fn hold_exclusive(token: &str, project_dir: Option<&Path>) -> std::io::Result<Hold> {
    hold(token, false, false, project_dir)
}

/// Attach a holder record to a freshly-taken flock. Registration is best-effort by design (see
/// [`holder::register`]): the hold is already ours, and losing the record costs diagnosis, not
/// exclusion.
fn registered(
    file: std::fs::File,
    path: &Path,
    token: &str,
    shared: bool,
    project_dir: Option<&Path>,
) -> Hold {
    Hold { record: holder::register(path, token, shared, project_dir), file }
}

/// Wait for a contended flock on a helper thread, narrating from this one.
///
/// **The blocking `flock` has to leave the calling thread**, because a thread inside it cannot
/// print: the old code went silent between "waiting" and "acquired", which over a 14 h wait meant
/// no line ever named the token, the holder, or the elapsed time (the run banks `run.lock_wait_ms`
/// only if it FINISHES). Keeping the kernel's wait rather than polling `try_hold` in a loop is
/// deliberate in an item about starvation: flock offers no fairness guarantee, but a barging poll
/// loop would actively remove what ordering the kernel does give.
fn wait_for_flock(
    file: std::fs::File,
    path: &Path,
    token: &str,
    shared: bool,
    policy: &WaitPolicy,
) -> std::io::Result<std::fs::File> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<std::fs::File>>();
    let signal = Arc::clone(&abandoned);
    std::thread::Builder::new().name(format!("prova-lock-{token}")).spawn(move || {
        let outcome = flock(&file, shared, true).map(|_| file);
        // Abandoning must never leak a hold: whoever gave up is gone, so a lock won after the
        // fact is released here rather than held by nobody. (The send-loses-the-race path is
        // equally safe — the receiver is dropped, the `File` rides back in the `SendError`, and
        // dropping it releases.)
        match outcome {
            Ok(f) if signal.load(Ordering::SeqCst) => drop(f),
            other => {
                let _ = tx.send(other);
            }
        }
    })?;

    let started = Instant::now();
    let mut next_line = policy.narrate_every.map(|every| (every, every));
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(outcome) => return outcome,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(std::io::Error::other(format!(
                    "lock {token:?}: the waiting thread ended without a verdict"
                )))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let waited = started.elapsed();
        if let Some((due, every)) = next_line {
            if waited >= due {
                eprintln!(
                    "prova: still waiting {} for lock {token:?} — {}",
                    holder::age(waited),
                    holder::describe_holders(path)
                );
                next_line = Some((due + every, every));
            }
        }
        if policy.bound.is_some_and(|bound| waited >= bound) {
            abandoned.store(true, Ordering::SeqCst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                // No token in the text: both callers already prefix the error with it, and
                // "lock \"cargo\": gave up waiting for lock \"cargo\"" reads as a bug.
                format!(
                    "gave up after {} — {}. Nothing can release another process's flock, so the \
                     exits are: wait it out, or end the holder (raise or remove the bound with \
                     PROVA_LOCK_WAIT_TIMEOUT; 0 waits forever)",
                    holder::age(waited),
                    holder::describe_holders(path)
                ),
            ));
        }
    }
}

/// One token's state, for the survey behind `prova locks`.
#[derive(Debug)]
pub struct LockStatus {
    /// The sanitized token, as the filename spells it.
    pub token: String,
    pub path: PathBuf,
    /// Whether anyone holds it right now — asked of the KERNEL, not of the records.
    pub held: bool,
    /// Who says they hold it. Empty against `held: true` is the unregistered-holder case.
    pub holders: Vec<holder::Entry>,
}

/// Every lock file in a scope, with who holds it.
///
/// Liveness comes from a non-blocking exclusive probe rather than from the records, because the
/// records are a hint and the flock is authority: a token held by a tool that never registered
/// must still report `held`. The probe takes and instantly releases the lock when it is free,
/// which is why it asks for the exclusive mode — a shared probe would succeed against a reader
/// and report a genuinely-held token as free.
pub fn survey(machine: bool, project_dir: Option<&Path>) -> Vec<LockStatus> {
    let dir = lock_dir(machine, project_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<LockStatus> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(token) = name.to_str().and_then(|n| n.strip_suffix(".lock")) else { continue };
        let path = entry.path();
        let held = match open_lock(&path).and_then(|f| flock(&f, false, false)) {
            Ok(acquired) => !acquired,
            // Unreadable is not free (docs/design/agent-ergonomics.md#unparseable-runstate-record-
            // reads-as-no-hold, the same fail-closed direction): a token we cannot probe is
            // reported as held rather than volunteered as available.
            Err(_) => true,
        };
        out.push(LockStatus {
            token: token.to_string(),
            held,
            holders: holder::read_all(&path),
            path,
        });
    }
    out.sort_by(|a, b| a.token.cmp(&b.token));
    out
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

    /// Taking a hold registers who took it, and releasing takes the record with it. The record's
    /// lifetime is the hold's — there is no release path that forgets it, because there is one
    /// release path.
    #[test]
    fn a_hold_names_its_holder_for_exactly_as_long_as_it_is_held() {
        let dir = crate::engine::make_tempdir().unwrap();
        let path = lock_path("cargo", false, Some(&dir)).unwrap();
        let held = try_hold("cargo", false, false, Some(&dir)).unwrap();
        assert!(held.is_some());
        match holder::read_all(&path).as_slice() {
            [holder::Entry::Live(h)] => {
                assert_eq!(h.pid, std::process::id());
                assert_eq!(h.token, "cargo");
                assert!(!h.shared, "a bare token is a writer");
            }
            other => panic!("the hold did not name itself: {other:?}"),
        }
        drop(held);
        assert!(holder::read_all(&path).is_empty(), "release takes the record with it");
    }

    /// The survey answers from the KERNEL, not from the sidecar. A token whose holder never
    /// registered — the external tool the convention explicitly invites — must read as held with
    /// nobody named, never as free.
    #[test]
    fn the_survey_reports_an_unregistered_holder_as_held() {
        let dir = crate::engine::make_tempdir().unwrap();
        let path = lock_path("foreign", false, Some(&dir)).unwrap();
        let raw = open_lock(&path).unwrap();
        assert!(flock(&raw, false, false).unwrap(), "the test's own hold, taken outside `try_hold`");

        match survey(false, Some(&dir)).as_slice() {
            [status] => {
                assert_eq!(status.token, "foreign");
                assert!(status.held, "the flock is authority");
                assert!(status.holders.is_empty(), "…and nobody registered");
            }
            other => panic!("expected one token, got {other:?}"),
        }
        drop(raw);
        let after = survey(false, Some(&dir));
        assert!(!after[0].held, "a released token reads as free");
    }

    /// A contended blocking hold under a bound gives up, names the holder, and leaves nothing
    /// held once the incumbent releases — the abandoned helper thread must not become a holder
    /// nobody is waiting on.
    #[test]
    fn a_bounded_wait_gives_up_naming_the_holder_and_leaks_no_hold() {
        let dir = crate::engine::make_tempdir().unwrap();
        let incumbent = try_hold("busy", false, false, Some(&dir)).unwrap();
        assert!(incumbent.is_some());

        let policy = WaitPolicy { bound: Some(Duration::from_millis(400)), narrate_every: None };
        let err = hold_with("busy", false, false, Some(&dir), &policy)
            .expect_err("a bounded wait behind a live holder must give up");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let text = err.to_string();
        assert!(text.contains("gave up after"), "{text}");
        assert!(!text.contains("busy"), "the caller supplies the token; saying it twice reads as a bug");
        assert!(text.contains(&format!("pid {}", std::process::id())), "names the holder: {text}");
        assert!(text.contains("PROVA_LOCK_WAIT_TIMEOUT"), "names the way out: {text}");

        // The helper thread wins the flock the moment this drops; the abandon flag must make it
        // let go rather than hold what nobody asked for.
        drop(incumbent);
        let mut reacquired = None;
        for _ in 0..100 {
            reacquired = try_hold("busy", false, false, Some(&dir)).unwrap();
            if reacquired.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(reacquired.is_some(), "the abandoned wait leaked a hold");
    }

    /// The two operator dials, in the one grammar. `0` is how "forever" / "never" is said
    /// deliberately, and a typo is an error the caller can report rather than a silent default.
    #[test]
    fn a_bound_is_a_duration_or_a_deliberate_zero() {
        assert_eq!(parse_bound("30m"), Ok(Some(Duration::from_secs(1800))));
        assert_eq!(parse_bound(" 2h "), Ok(Some(Duration::from_secs(7200))));
        for forever in ["0", "off", "never", "", "  "] {
            assert_eq!(parse_bound(forever), Ok(None), "{forever:?} means no bound");
        }
        assert!(parse_bound("soon").is_err(), "a typo is not silently 'forever'");
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
