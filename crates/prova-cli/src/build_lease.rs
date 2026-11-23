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

/// A held lease, released when dropped or when the process ends.
pub(crate) struct Held {
    _file: Option<File>,
}

/// Hold the build dir's lease for this run, if the run is in a cargo project. Never fails the run.
pub(crate) fn hold() -> Held {
    let Some(dir) = build_dir() else {
        return Held { _file: None };
    };
    match hold_at(&dir) {
        Ok(file) => Held { _file: Some(file) },
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
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LEASE))?;
    match file.try_lock_shared() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            eprintln!(
                "prova: the build volume's sweep holds {} now; waiting for it",
                dir.display()
            );
            file.lock_shared()?;
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(e),
    }
    Ok(file)
}

/// The cargo build dir of the project at the current directory: `build_directory`, else `target_directory`, as cargo
/// itself says. None outside a cargo project or when cargo can't answer.
fn build_dir() -> Option<PathBuf> {
    let here = std::env::current_dir().ok()?;
    if !here.ancestors().any(|d| d.join("Cargo.toml").is_file()) {
        return None;
    }
    let out = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let pick = |k: &str| {
        meta.get(k)
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
    };
    pick("build_directory").or_else(|| pick("target_directory"))
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
}
