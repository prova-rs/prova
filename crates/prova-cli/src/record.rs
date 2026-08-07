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
