//! A run holds its cargo build dir's lease for its whole life (E m/28458, A m/28455).
//!
//! The build volume's sweep (anemnez-ecosystem's `space::strategy`) treats a cargo build dir as busy while its
//! `.space-lease` is flocked, before it even reads cargo's own build locks. Cargo's lock covers only a build: nextest
//! runs the test binaries after cargo has exited, so a dir under a running suite read idle and was deleted under it
//! ("[double-spawn] failed to exec" on hydramq-replication, the Studio, 2026-11-23 22:08Z). So a run that can build
//! takes a SHARED flock on `<build dir>/.space-lease` before it starts and keeps it until the process ends: two runs
//! share it, the sweep's exclusive take fails while either holds it, and a crash lets go with the process. It's the
//! same lease file the sweep reads, never a second one.
//!
//! Best effort and said: a project with no Cargo.toml, or a cargo that can't answer, runs as before, with a line.

use std::fs::File;
use std::path::{Path, PathBuf};

/// The lease file the sweep holds a cargo build dir by (anemnez-ecosystem `space::strategy::LEASE`).
pub(crate) const LEASE: &str = ".space-lease";

/// How long a run waits for the sweep to let go of a dir it is deleting, before it goes on unprotected and says so.
const SWEEP_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long `cargo metadata` may take to name the build dir before the run goes on without the lease.
const METADATA_WITHIN: std::time::Duration = std::time::Duration::from_secs(60);

/// How often a bounded wait looks again.
const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// A held lease, released when dropped or when the process ends.
pub(crate) struct Held {
    _file: Option<File>,
}

/// Hold the build dir's lease for this run, if the run is in a cargo project. Never fails the run.
pub(crate) fn hold() -> Held {
    let dir = match build_dir() {
        Ok(Some(dir)) => dir,
        Ok(None) => return Held { _file: None },
        Err(why) => {
            // Said, never silent (E m/33453): a run with no lease reads to the sweep as an idle dir, so a gate whose
            // binaries vanish mid-run must be able to tell this from the sweep ignoring a held lease.
            eprintln!("prova: the build dir's lease is not held: {why}; the sweep may reclaim it mid-run");
            return Held { _file: None };
        }
    };
    match hold_at(&dir) {
        Ok(file) => {
            // Said once, with the path (L m/33465): the sweep honours a held lease, so a gate that loses its binaries
            // mid-run reads here whether this run held the dir the sweep deleted, by value.
            eprintln!("prova: holding the build dir's lease at {}", dir.join(LEASE).display());
            Held { _file: Some(file) }
        }
        Err(e) => {
            eprintln!(
                "prova: the build dir's lease ({}) could not be held: {e}; the sweep may reclaim it mid-run",
                dir.display()
            );
            Held { _file: None }
        }
    }
}

/// Take a shared flock on `<dir>/.space-lease`, waiting while the sweep holds it (it's deleting this dir now).
pub(crate) fn hold_at(dir: &Path) -> std::io::Result<File> {
    hold_within(dir, SWEEP_WAIT)
}

/// [`hold_at`] with its bound handed in, so a row can prove the wait ends.
///
/// A granted lock counts only while the lease file it's on is still the one at `<dir>/.space-lease` (L m/34435): the
/// sweep holds the lease exclusively while it renames the dir into its trash, so a run that waited on the old fd is
/// granted its lock on the TRASHED file, while cargo recreates `<dir>` with a lease nobody holds, and every later round
/// takes it and deletes the dir under the run. So each round opens the path afresh, and a lock whose file is no longer
/// the path's is dropped and taken again, all within `wait`.
fn hold_within(dir: &Path, wait: std::time::Duration) -> std::io::Result<File> {
    let started = std::time::Instant::now();
    let mut waiting = false;
    loop {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(LEASE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        match file.try_lock_shared() {
            Ok(()) if is_the_path(&file, &path) => return Ok(file),
            Ok(()) => eprintln!(
                "prova: the lease at {} moved away as it was granted (the sweep took the old dir); taking the new one",
                path.display()
            ),
            Err(std::fs::TryLockError::WouldBlock) if !waiting => {
                waiting = true;
                // Rule 4: the wait is bounded. A sweep deletes one dir in seconds; past SWEEP_WAIT the run goes on
                // unprotected, said, never waits for good.
                eprintln!("prova: the build volume's sweep holds {} now; waiting for it", dir.display());
                #[cfg(test)]
                tests::waiting_now();
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        if started.elapsed() >= wait {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("the sweep still held it after {}ms", wait.as_millis()),
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// Whether `file` is still the file at `path`: the same device and inode. A file renamed away (into the sweep's trash)
/// or replaced is not.
#[cfg(unix)]
fn is_the_path(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(there)) => (held.dev(), held.ino()) == (there.dev(), there.ino()),
        _ => false,
    }
}

/// Off unix (Windows is parked) a file in use can't be renamed away, so the file is the path's.
#[cfg(not(unix))]
fn is_the_path(_: &File, _: &Path) -> bool {
    true
}

/// The cargo build dir of the project at the current directory: `build_directory`, else `target_directory`, as cargo
/// itself says. `Ok(None)` outside a cargo project; an error, naming why, when cargo can't answer.
fn build_dir() -> Result<Option<PathBuf>, String> {
    let here = std::env::current_dir().map_err(|e| format!("no current directory: {e}"))?;
    if !here.ancestors().any(|d| d.join("Cargo.toml").is_file()) {
        return Ok(None);
    }
    // Rule 4: cargo is bounded too. A hung cargo must never hold a run before its first proof.
    let mut child = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cargo metadata didn't start: {e}"))?;
    // Read stdout as it comes, on its own thread: a workspace's metadata can pass a pipe's buffer, and a cargo
    // blocked writing it would never exit, which the bound would misread as a hang.
    let mut pipe = child.stdout.take().ok_or("cargo metadata gave no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut pipe, &mut out).map(|_| out)
    });
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|e| format!("cargo metadata: {e}"))? {
            Some(status) if status.success() => break,
            Some(status) => return Err(format!("cargo metadata --offline exited {status}")),
            None if started.elapsed() >= METADATA_WITHIN => {
                drop(child.kill());
                drop(child.wait());
                return Err(format!("cargo metadata didn't answer in {}s", METADATA_WITHIN.as_secs()));
            }
            None => std::thread::sleep(POLL),
        }
    }
    let stdout = reader
        .join()
        .map_err(|_| "cargo metadata's reader panicked".to_owned())?
        .map_err(|e| format!("cargo metadata's output: {e}"))?;
    let meta: serde_json::Value =
        serde_json::from_slice(&stdout).map_err(|e| format!("cargo metadata's JSON: {e}"))?;
    let pick = |k: &str| {
        meta.get(k)
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
    };
    pick("build_directory")
        .or_else(|| pick("target_directory"))
        .map(Some)
        .ok_or_else(|| "cargo metadata named neither build_directory nor target_directory".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sweep's own take (an exclusive, non-blocking flock on the lease, as `space::strategy::Generic::hold`).
    fn sweep_takes(dir: &Path) -> bool {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LEASE))
            .unwrap_or_else(|e| panic!("{e}"));
        file.try_lock().is_ok()
    }

    /// E m/33453: a cargo project whose metadata can't be read gives its reason, never a silent None, so a run with
    /// no lease says so (Herald's gate at 091bc3e lost its test binaries to the sweep, and nothing said whether the
    /// lease was ever held). Outside a cargo project there is nothing to hold, and nothing is said. nextest runs each
    /// test in its own process, so the directory change is this row's alone.
    #[test]
    fn a_build_dir_cargo_cant_name_is_said_never_silent() {
        let dir = std::env::temp_dir().join(format!("prova-build-lease-said-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("Cargo.toml"), "this is not a manifest [[[").unwrap_or_else(|e| panic!("{e}"));
        std::env::set_current_dir(&dir).unwrap_or_else(|e| panic!("{e}"));
        let why = build_dir().expect_err("a manifest cargo refuses names why");
        assert!(why.contains("cargo metadata"), "{why}");
        let outside = std::env::temp_dir().join(format!("prova-build-lease-none-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap_or_else(|e| panic!("{e}"));
        std::env::set_current_dir(&outside).unwrap_or_else(|e| panic!("{e}"));
        if !outside.ancestors().any(|d| d.join("Cargo.toml").is_file()) {
            assert_eq!(build_dir(), Ok(None), "no cargo project: nothing to hold, nothing said");
        }
        drop(std::fs::remove_dir_all(&dir));
        drop(std::fs::remove_dir_all(&outside));
    }

    /// E m/28458: while a run holds its build dir's lease the sweep can't take it, two runs share it, and once both
    /// end the sweep can.
    #[test]
    fn a_run_holds_its_build_dir_from_the_sweep_and_two_runs_share_it() {
        let dir = std::env::temp_dir().join(format!("prova-build-lease-{}", std::process::id()));
        let one = hold_at(&dir).unwrap_or_else(|e| panic!("{e}"));
        let two = hold_at(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert!(!sweep_takes(&dir), "busy while a run holds it");
        drop(one);
        assert!(
            !sweep_takes(&dir),
            "still busy while the second run holds it"
        );
        drop(two);
        // A child another test thread forks mid-spawn holds a copy of every fd until its exec (machine_slot's retake,
        // E m/14323), so a just-dropped lock may still read held for that moment: a few looks, never one.
        assert!(
            (0..50).any(|_| sweep_takes(&dir) || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                false
            }),
            "free once every run has let go"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// D m/28476: a child the run spawns and that outlives it never keeps the dir unsweepable. std opens the lease
    /// O_CLOEXEC, so an exec'd child holds no copy of it: once the run lets go the sweep can take the dir while the
    /// child still runs.
    #[test]
    fn a_child_that_outlives_the_run_does_not_hold_its_lease() {
        let dir = std::env::temp_dir().join(format!("prova-build-lease-child-{}", std::process::id()));
        let lease = hold_at(&dir).unwrap_or_else(|e| panic!("{e}"));
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap_or_else(|e| panic!("{e}"));
        drop(lease);
        let freed = (0..50).any(|_| {
            sweep_takes(&dir) || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                false
            }
        });
        let still_running = child.try_wait().ok().flatten().is_none();
        drop(child.kill());
        drop(child.wait());
        drop(std::fs::remove_dir_all(&dir));
        assert!(still_running, "the child outlived the run's lease");
        assert!(freed, "the sweep takes the dir while the run's child still runs");
    }

    /// A row's seam, by value (D m/34459): the waiting run says it's waiting here, so a row moves the dir only once the
    /// run is in its wait, never after a sleep that hopes it is.
    static WAITING: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> = std::sync::Mutex::new(None);

    pub(super) fn waiting_now() {
        if let Some(tx) = WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            let _ = tx.send(());
        }
    }

    /// L m/34435: the sweep holds the lease exclusively, renames the dir into its trash, and lets go. A run that was
    /// waiting ends holding the lease at the PATH, the dir cargo recreates, never the trashed file, so the sweep's next
    /// take of that dir is refused. The dir moves once the run says it's waiting (by value). Red with the old wait,
    /// whose lock landed on the trashed inode.
    #[test]
    fn a_run_waiting_while_the_sweep_moves_the_dir_holds_the_new_lease() {
        let root = std::env::temp_dir().join(format!("prova-build-lease-moved-{}", std::process::id()));
        let (dir, trash) = (root.join("unit"), root.join("trash"));
        drop(std::fs::remove_dir_all(&root));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let sweep = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LEASE))
            .unwrap_or_else(|e| panic!("{e}"));
        sweep.lock().unwrap_or_else(|e| panic!("{e}"));
        let (tx, rx) = std::sync::mpsc::channel();
        *WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
        let waiter = {
            let dir = dir.clone();
            std::thread::spawn(move || hold_within(&dir, std::time::Duration::from_secs(20)))
        };
        // The run is in its wait on the held lease (it said so): the sweep moves the dir away while it holds it, then
        // lets go.
        rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap_or_else(|e| panic!("the run never waited: {e}"));
        std::fs::rename(&dir, &trash).unwrap_or_else(|e| panic!("{e}"));
        drop(sweep);
        let held = waiter.join().unwrap_or_else(|_| panic!("the waiter panicked")).unwrap_or_else(|e| panic!("{e}"));
        let on_path = is_the_path(&held, &dir.join(LEASE));
        let swept_again = dir.exists() && sweep_takes(&dir);
        drop(held);
        drop(std::fs::remove_dir_all(&root));
        assert!(on_path, "the run holds the lease at the path, never the trashed file");
        assert!(!swept_again, "the sweep's next take of the recreated dir is refused");
    }

    /// The other half of L m/34435: a lease file the sweep renamed between a round's open and its lock is no longer the
    /// path's, so a lock granted on it is never counted. (A run already waiting reopens each round and never holds the
    /// old file; this is the one window the check closes.) Red if the check compared only the path.
    #[test]
    fn a_lease_file_renamed_away_is_not_the_paths() {
        let root = std::env::temp_dir().join(format!("prova-build-lease-renamed-{}", std::process::id()));
        let (dir, trash) = (root.join("unit"), root.join("trash"));
        drop(std::fs::remove_dir_all(&root));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let opened = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LEASE))
            .unwrap_or_else(|e| panic!("{e}"));
        let before = is_the_path(&opened, &dir.join(LEASE));
        std::fs::rename(&dir, &trash).unwrap_or_else(|e| panic!("{e}"));
        let gone = is_the_path(&opened, &dir.join(LEASE));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join(LEASE), b"").unwrap_or_else(|e| panic!("{e}"));
        let recreated = is_the_path(&opened, &dir.join(LEASE));
        drop(opened);
        drop(std::fs::remove_dir_all(&root));
        assert!(before, "the file opened at the path is the path's");
        assert!(!gone && !recreated, "renamed away, it's no longer the path's, nor once the path is recreated");
    }

    /// L m/28480, rule 4: while the sweep holds the lease, a run's wait for it ends at its bound with TimedOut, said,
    /// and never waits for good.
    #[test]
    fn a_runs_wait_for_the_sweep_ends_at_its_bound() {
        let dir = std::env::temp_dir().join(format!("prova-build-lease-bound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let sweep = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LEASE))
            .unwrap_or_else(|e| panic!("{e}"));
        sweep.lock().unwrap_or_else(|e| panic!("{e}"));
        let got = hold_within(&dir, std::time::Duration::from_millis(300));
        drop(sweep);
        drop(std::fs::remove_dir_all(&dir));
        assert_eq!(got.err().map(|e| e.kind()), Some(std::io::ErrorKind::TimedOut), "ended at its bound");
    }
}
