//! The run record — what a run actually executed, and what it did not.
//!
//! `0 failed` is the last thing an *honest* agent can say that is true and worthless. It is true of
//! a run in which every proof skipped for want of a docker daemon, true of a run whose selection
//! matched three tests out of four hundred, and true of a run that collected nothing at all. The
//! failure count answers "did anything go red"; nobody was asking that. They were asking "is this
//! covered", and the negative space — the skipped, the deselected — is where that answer lives.
//!
//! So each run writes down what it did. Counts, and then the individual paths behind the two counts
//! that mean *no evidence was produced*. [`attest`] reads it back and answers the question about one
//! obligation: not "did the suite pass" but "did the proof for THIS claim actually run".
//!
//! ## Deliberately not signed
//!
//! The threat model is a careless agent, not a malicious one. An agent that would forge a record
//! would equally write a falsifier that mutates nothing, and a signature buys ceremony and key
//! management rather than truth. The honest ceiling here is "an agent cannot be wrong by accident".
//!
//! ## Where it lives
//!
//! `<home>/.prova/var/last-run.json` on every run — prova's own self-ignoring state directory, so it
//! costs the user no ignore-file edits and never lands in a tracked tree. `--record <path>` *also*
//! emits it wherever asked for, for CI to keep as an artifact or a bot to post; the var/ copy is for the
//! next command, the emitted one is for a human.
//!
//! ## Types
//!
//! The canonical record types live in `prova_core::ledger`; the CLI is one renderer over that
//! ledger, not its owner. This module keeps the CLI-side policy: where the record lives, how to read
//! and write it, and the conversions from engine types into record rows.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::home::Home;
use crate::var;

/// The record's basename in `var/`. No leading dot: it already sits in a hidden, self-ignoring
/// directory, and a second layer of hiding only makes it harder to read while debugging.
pub const LAST_RUN: &str = "last-run.json";

// The canonical run-record types live in the core ledger so embeddings can read the same file.
pub use prova_core::ledger::{
    attest, Attested, Counts, DeputedRow, Executed, MeasurementRow, Record, ReminderEntry, Skipped,
};

/// Convert the engine's evaluated reminders into record rows.
pub fn reminder_entries(outcomes: &[prova_core::ReminderOutcome]) -> Vec<ReminderEntry> {
    use prova_core::ReminderState;
    outcomes
        .iter()
        .map(|o| {
            let (state, why) = match &o.state {
                ReminderState::Watching => ("watching", None),
                ReminderState::Due { why } => ("due", why.clone()),
                ReminderState::Unevaluated { reason } => ("unevaluated", Some(reason.clone())),
            };
            ReminderEntry {
                name: o.name.clone(),
                state: state.to_string(),
                why,
                message: o.message.clone(),
                tags: o.tags.clone(),
                file: o.file.clone(),
                line: o.line,
            }
        })
        .collect()
}

/// Convert the engine's ingested cases into record rows.
pub fn deputed_rows(cases: &[prova_core::DeputedCase]) -> Vec<DeputedRow> {
    cases
        .iter()
        .map(|c| DeputedRow {
            verifier: c.verifier.clone(),
            suite: c.suite.clone(),
            name: c.name.clone(),
            outcome: c.outcome.clone(),
            message: c.message.clone(),
            time_ms: c.time_ms,
            file: c.file.clone(),
        })
        .collect()
}

/// Convert the engine's recorded measurements into record rows.
pub fn measurement_rows(measurements: &[prova_core::Measurement]) -> Vec<MeasurementRow> {
    measurements
        .iter()
        .map(|m| MeasurementRow {
            name: m.name.clone(),
            value: m.value,
            // The stable string form lives on the core type, shared with the baseline file.
            direction: m.direction.as_str().to_string(),
            set: m.set.clone(),
        })
        .collect()
}

/// Which build produced a record.
///
/// A content hash of the executable would be the obvious answer and is the wrong one: prova's own
/// binary is hundreds of megabytes unoptimized, and hashing it on *every* run would put a visible
/// tax on the ordinary path — which is exactly how machinery gets switched off. Version plus the
/// executable's size and mtime distinguishes every build that a developer or CI will actually
/// produce, at the cost of one `stat`.
pub fn binary_fingerprint() -> String {
    let mut hasher = <Sha256 as Digest>::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    if let Ok(exe) = std::env::current_exe() {
        hasher.update(exe.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(&exe) {
            hasher.update(meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified() {
                if let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) {
                    hasher.update(since.as_secs().to_le_bytes());
                }
            }
        }
    }
    hex::encode(hasher.finalize())[..12].to_string()
}

/// A leaf's path, qualified by the file it was declared in — the record's key form.
///
/// One definition, in core ([`prova_core::qualify_leaf_path`]), because the deselected are
/// qualified there (they emit no event to carry a file) and the executed and skipped are qualified
/// here. Two spellings of one address would put a proof in the record under a name `attest` could
/// not find.
pub fn qualified(path: &str, file: Option<&str>) -> String {
    prova_core::qualify_leaf_path(path, file.map(Path::new))
}

/// The record path for a package. Resolves without creating anything, so a read on a package that
/// has never run leaves no directory behind.
pub fn path(home: &Home) -> PathBuf {
    var::path(home).join(LAST_RUN)
}

/// Read the last run's record, if one exists and parses.
pub fn load(home: &Home) -> Option<prova_core::ledger::Record> {
    prova_core::ledger::read_record(&path(home)).ok()
}

/// Write the record into `var/` (materializing it), and to `also` when `--record` asked for a copy.
///
/// Best-effort by construction: a run's verdict must never depend on whether its record could be
/// written. A package with no manifest home has nowhere durable to put one and quietly records
/// nothing, exactly as `--last-failed` does.
pub fn store(home: &Option<Home>, record: &Record, also: Option<&Path>) {
    let Ok(text) = serde_json::to_string_pretty(record) else {
        return;
    };
    if let Some(home) = home.as_ref() {
        if let Ok(dir) = var::dir(home) {
            let _ = std::fs::write(dir.join(LAST_RUN), &text);
        }
    }
    if let Some(dest) = also {
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(dest, &text) {
            // The var/ copy is a byproduct; an explicitly requested one is an instruction, and
            // silently not honoring it would be its own small lie.
            eprintln!("prova: --record: could not write {}: {e}", dest.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prova-record-ut-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home_in(dir: &Path) -> Home {
        Home {
            dir: dir.to_path_buf(),
            manifest: dir.join("prova.toml"),
        }
    }

    fn minimal_record() -> Record {
        Record {
            schema: 1,
            version: "0.0.0-test".into(),
            binary: "test".into(),
            selection: vec!["engine".into()],
            duration_ms: 7,
            summary: Counts {
                passed: 2,
                ..Counts::default()
            },
            executed: BTreeMap::from([("f › t".to_string(), Executed::Passed)]),
            skipped: vec![],
            deselected: vec!["f › other".into()],
            measurements: vec![],
            attached: vec![],
            reminders: vec![],
            deputed: vec![],
        }
    }

    /// The record's key form: the declaring file's stem prefixes the path, unless the path
    /// already leads with it — the rule that keeps executed/skipped (qualified here) and
    /// deselected (qualified in core) under ONE spelling per address.
    #[test]
    fn qualified_prefixes_the_file_stem_exactly_once() {
        assert_eq!(qualified("a › b", Some("proofs/x.lua")), "x › a › b");
        assert_eq!(qualified("x › a", Some("proofs/x.lua")), "x › a");
        assert_eq!(qualified("a › b", None), "a › b");
    }

    /// Each mapper carries the engine's fields through by name; the reminder mapper also owns
    /// the state→string spelling, which the record file then freezes.
    #[test]
    fn row_mappers_carry_engine_fields_and_spell_reminder_states() {
        use prova_core::{ReminderOutcome, ReminderState};
        let spell = |state: ReminderState| {
            let rows = reminder_entries(&[ReminderOutcome {
                name: "n".into(),
                message: "do the thing".into(),
                tags: vec!["ops".into()],
                file: Some("r.lua".into()),
                line: Some(3),
                state,
            }]);
            (rows[0].state.clone(), rows[0].why.clone())
        };
        assert_eq!(spell(ReminderState::Watching), ("watching".into(), None));
        assert_eq!(
            spell(ReminderState::Due { why: Some("date passed".into()) }),
            ("due".into(), Some("date passed".into()))
        );
        assert_eq!(
            spell(ReminderState::Unevaluated { reason: "no clock".into() }),
            ("unevaluated".into(), Some("no clock".into()))
        );

        let deputed = deputed_rows(&[prova_core::DeputedCase {
            verifier: "junit".into(),
            suite: "SuiteA".into(),
            name: "case_1".into(),
            outcome: "failed".into(),
            message: Some("boom".into()),
            time_ms: Some(12),
            file: "target/junit.xml".into(),
        }]);
        assert_eq!(deputed[0].verifier, "junit");
        assert_eq!(deputed[0].outcome, "failed");
        assert_eq!(deputed[0].time_ms, Some(12));

        let measured = measurement_rows(&[prova_core::Measurement {
            name: "rust.coverage.unit".into(),
            value: 64.9,
            direction: prova_core::Direction::HigherIsBetter,
            set: "quality".into(),
        }]);
        assert_eq!(measured[0].direction, "higher_is_better");
        assert_eq!(measured[0].set, "quality");
    }

    /// The full lifecycle in a tempdir: store materializes var/, load reads the same record
    /// back, a corrupt file reads as None (never an error), and an explicit `--record` copy
    /// lands even with no home to put the var/ copy in.
    #[test]
    fn store_then_load_round_trips_and_corruption_reads_as_none() {
        let dir = tempdir("roundtrip");
        let home = home_in(&dir);

        assert!(load(&home).is_none(), "a never-run package has no record");

        let record = minimal_record();
        store(&Some(home.clone()), &record, None);
        let back = load(&home).expect("stored record loads");
        assert_eq!(back.summary.passed, 2);
        assert_eq!(back.deselected, vec!["f › other".to_string()]);
        assert!(matches!(back.executed["f › t"], Executed::Passed));

        std::fs::write(path(&home), "{ not json").unwrap();
        assert!(load(&home).is_none(), "a corrupt record loads as None");

        let also = dir.join("artifacts").join("run.json");
        store(&None, &record, Some(&also));
        assert!(also.is_file(), "--record writes even with no package home");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
