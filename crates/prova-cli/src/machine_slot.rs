//! Machine slots: a heavy run waits for one of the machine's N slots before it starts, so the
//! machine's heavy work (several projects' full gates, a release pipeline's gate, a stress run, a
//! service booting) never all lands on its cores and disk at once.
//!
//! The protocol is files, shared with any other program on the machine that honours it (Anemnez's
//! Herald and Fabric do): the slots are `slot-0.lock` … `slot-<N-1>.lock` in one directory, N is
//! the integer in its `slots` file (else 3), a taker holds an exclusive `flock` on any one slot's
//! file for its whole run and writes who it is into it, and the kernel frees the slot when the
//! process ends, however it ends. Nothing to configure for a machine that has no such directory:
//! a project opts in with `[machine] slots = "<dir>"` in prova.toml, or the environment names it
//! (`PROVA_MACHINE_SLOTS`). `prova run <lane>` and `prova stress` take a slot; the inner loop does
//! not. A process whose caller already holds a slot for it (`PROVA_MACHINE_SLOT_HELD`, which a
//! run sets for its own children) takes none, so nested runs never count twice or deadlock.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Set for a run's children: their slot is already held.
pub(crate) const HELD: &str = "PROVA_MACHINE_SLOT_HELD";
/// The slots directory, when the environment names it (it wins over prova.toml).
pub(crate) const DIR_ENV: &str = "PROVA_MACHINE_SLOTS";
/// Slots when the directory's `slots` file names none.
pub(crate) const DEFAULT_SLOTS: usize = 3;
/// How often a waiting run looks again: a sampling rate, never a budget.
const LOOK_AGAIN: Duration = Duration::from_secs(2);

/// A held slot (or none taken); dropping it frees the slot.
pub(crate) struct Held {
    _slot: Option<File>,
}

/// The slots directory a manifest's text declares (`[machine] slots`), relative paths against `home`.
pub(crate) fn declared(manifest: &str, home: &Path) -> Option<PathBuf> {
    let value: toml::Value = manifest.parse().ok()?;
    let dir = value.get("machine")?.get("slots")?.as_str()?;
    let dir = PathBuf::from(dir);
    Some(if dir.is_absolute() {
        dir
    } else {
        home.join(dir)
    })
}

fn count(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("slots"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_SLOTS)
        .max(1)
}

/// A free slot in `dir` now, naming `who`, or `None` when every slot is held.
pub(crate) fn try_take(dir: &Path, who: &str) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(dir)?;
    for index in 0..count(dir) {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(format!("slot-{index}.lock")))?;
        match file.try_lock() {
            Ok(()) => {
                file.set_len(0)?;
                writeln!(file, "{who} pid={}", std::process::id())?;
                return Ok(Some(file));
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
    Ok(None)
}

/// Take a slot for `who` if this machine has slots and no caller holds one for this process;
/// waits, saying so once, while every slot is held. Children inherit `HELD`.
pub(crate) fn take(who: &str) -> Held {
    if std::env::var_os(HELD).is_some() {
        return Held { _slot: None };
    }
    let dir = std::env::var_os(DIR_ENV).map(PathBuf::from).or_else(|| {
        let cwd = std::env::current_dir().ok()?;
        let home = crate::home::find(&cwd).ok()??;
        declared(&std::fs::read_to_string(&home.manifest).ok()?, &home.dir)
    });
    let Some(dir) = dir else {
        return Held { _slot: None };
    };
    let mut said = false;
    let file = loop {
        match try_take(&dir, who) {
            Ok(Some(file)) => break Some(file),
            Ok(None) => {
                if !said {
                    eprintln!(
                        "prova: every machine slot in {} is held; waiting for one",
                        dir.display()
                    );
                    said = true;
                }
                std::thread::sleep(LOOK_AGAIN);
            }
            Err(e) => {
                eprintln!(
                    "prova: machine slots in {} unusable ({e}); running without one",
                    dir.display()
                );
                break None;
            }
        }
    };
    std::env::set_var(HELD, "1");
    Held { _slot: file }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_has_n_slots_and_a_dropped_one_is_free_again() {
        let dir = std::env::temp_dir().join(format!("prova-slots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("slots"), "2").unwrap();
        let a = try_take(&dir, "a").unwrap().expect("first");
        let b = try_take(&dir, "b").unwrap().expect("second");
        assert!(
            try_take(&dir, "c").unwrap().is_none(),
            "two slots, both held"
        );
        drop(a);
        assert!(
            try_take(&dir, "c").unwrap().is_some(),
            "a's slot is free once a lets go"
        );
        drop(b);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_project_declares_its_machine_slots_and_a_relative_dir_is_its_homes() {
        let home = Path::new("/p");
        assert_eq!(
            declared("[machine]\nslots = \"/tmp/x\"\n", home),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(
            declared("[machine]\nslots = \"s\"\n", home),
            Some(PathBuf::from("/p/s"))
        );
        assert_eq!(
            declared("[run]\nproofs = [\"proofs\"]\n", home),
            None,
            "no section, no slots"
        );
    }
}
