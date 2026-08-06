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
//! emits it wherever asked, for CI to keep as an artifact or a bot to post; the var/ copy is for the
//! next command, the emitted one is for a human.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::home::Home;
use crate::var;

/// The record's basename in `var/`. No leading dot: it already sits in a hidden, self-ignoring
/// directory, and a second layer of hiding only makes it harder to read while debugging.
pub const LAST_RUN: &str = "last-run.json";

/// What became of one leaf that actually ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Executed {
    Passed,
    Failed,
    /// An open promise: red by definition, and therefore not evidence of anything working.
    Spec,
}

/// A leaf that ran into a gate before its body — and the gate's own words for why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    /// The skip reason as reported (`requires "docker" (…unavailable)`, a failed dependency, …).
    /// Verbatim, because a reason paraphrased by the recorder is a reason nobody can act on.
    pub reason: String,
}

/// Run totals. Mirrors `prova_core::Summary`, minus the parts that are not durable facts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub spec: usize,
    pub deselected: usize,
}

/// One reminder, as the run evaluated it — the attention account's row in the record
/// (docs/design/reminders.md). Conditions evaluate during RUNS; the query verbs (`reminders`,
/// `owed`, `evidence`) execute nothing and read these rows back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderEntry {
    pub name: String,
    /// `"watching"` | `"due"` | `"unevaluated"`. A string, not an enum: the record is a wire format
    /// other tools read, and an unknown future state should render as itself, not fail the parse.
    pub state: String,
    /// For `due`: the condition's own report of what the world did. For `unevaluated`: the reason
    /// it could not look. Absent for `watching`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The instruction — what to do when this fires.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl ReminderEntry {
    pub fn is_due(&self) -> bool {
        self.state == "due"
    }
}

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

/// One deputed case — a verdict another verifier produced, conducted by a proof and adopted
/// into this run's account (docs/design/verifiers.md). `prova attest junit:<suite>#<name>`
/// answers against these rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeputedRow {
    /// Which verifier produced it (`"junit"` today).
    pub verifier: String,
    pub suite: String,
    pub name: String,
    /// The deputy's own vocabulary, kept: `"passed"` | `"failed"` | `"error"` | `"skipped"`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_ms: Option<u64>,
    /// The artifact file the verdict was read from — the provenance of the adoption.
    pub file: String,
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

/// One recorded measurement — a named scalar this run observed, with which way is better and which
/// baseline set it belongs to. History for the record, and the source `--update-baseline` reads
/// when asked to move a baseline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasurementRow {
    pub name: String,
    pub value: f64,
    /// `"lower_is_better"` | `"higher_is_better"`.
    pub direction: String,
    /// The baseline set this belongs to (`.prova/baselines/<set>.json`).
    pub set: String,
}

/// The stable string form of a direction, shared by the record row and the baseline file.
pub fn direction_str(d: prova_core::Direction) -> &'static str {
    match d {
        prova_core::Direction::LowerIsBetter => "lower_is_better",
        prova_core::Direction::HigherIsBetter => "higher_is_better",
    }
}

/// Convert the engine's recorded measurements into record rows.
pub fn measurement_rows(measurements: &[prova_core::Measurement]) -> Vec<MeasurementRow> {
    measurements
        .iter()
        .map(|m| MeasurementRow {
            name: m.name.clone(),
            value: m.value,
            direction: direction_str(m.direction).to_string(),
            set: m.set.clone(),
        })
        .collect()
}

/// One run, as a durable fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Schema version of this file, so a future reader can refuse an unfamiliar shape rather than
    /// silently misread it.
    pub schema: u32,
    /// The prova version that produced it.
    pub version: String,
    /// Which build of the binary produced it — see [`binary_fingerprint`].
    pub binary: String,
    /// How the run was narrowed, spelled as it was asked for. Empty means "everything".
    pub selection: Vec<String>,
    pub duration_ms: u64,
    pub summary: Counts,
    /// Every leaf that ran, and what became of it.
    pub executed: BTreeMap<String, Executed>,
    /// Named, not summed — the whole point of the record.
    pub skipped: Vec<Skipped>,
    /// Named, not summed. Deselection and skipping have different causes and one consequence.
    pub deselected: Vec<String>,
    /// The attention account: every reminder with its evaluated state. Defaulted so records from
    /// before reminders existed still parse; a filtered run carries the previous run's rows forward
    /// (conditions are only sound against the FULL account, and a `-k` run must not wipe them).
    #[serde(default)]
    pub reminders: Vec<ReminderEntry>,
    /// The deputed account: every case a verifier facet ingested this run, with provenance.
    #[serde(default)]
    pub deputed: Vec<DeputedRow>,
    /// The measurement account: every scalar a `measure.record`/`measure.ratchet` call took this
    /// run, with its direction and baseline set. Defaulted so records from before it existed parse.
    #[serde(default)]
    pub measurements: Vec<MeasurementRow>,
    /// Held topologies this run ATTACHED to instead of provisioning
    /// (docs/design/topologies.md#attach-is-recorded). Live-state evidence is legitimately weaker
    /// than hermetic evidence — the environment was not built by this run and may carry state from
    /// prior ones — so the provenance is on the record for `attest`/`evidence` to weigh. Defaulted
    /// so records from before attach existed still parse.
    #[serde(default)]
    pub attached: Vec<String>,
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

/// The record path for a package. Resolves without creating anything, so a read on a package that
/// has never run leaves no directory behind.
pub fn path(home: &Home) -> PathBuf {
    var::path(home).join(LAST_RUN)
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

/// Read the last run's record, if one exists and parses.
pub fn load(home: &Home) -> Option<Record> {
    let text = std::fs::read_to_string(path(home)).ok()?;
    serde_json::from_str(&text).ok()
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

/// The verdict for one obligation, against one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attested {
    /// A covering proof ran and passed.
    Yes { path: String },
    /// A covering proof ran and did not pass.
    Red { path: String, outcome: Executed },
    /// A covering proof exists, and did not run.
    NoEvidence { path: String, why: String },
    /// Nothing claims to discharge this address at all.
    Unbound,
}

impl Attested {
    /// Only an executed, passing proof attests. Everything else — red, skipped, deselected, absent,
    /// unbound — is the absence of evidence, and exiting 0 on "I found nothing to check" is the
    /// vacuous pass this whole line of work exists to refuse.
    pub fn is_attested(&self) -> bool {
        matches!(self, Attested::Yes { .. })
    }
}

/// Reconcile one obligation address against a run record.
///
/// `bindings` are the node paths of the proofs that claim to cover the address (pins already
/// stripped by the caller — a pin says which prose was accepted, not which proof ran).
///
/// Resolved worst-first: if ANY covering proof failed to produce evidence, the obligation is not
/// attested, even when a sibling passed. Two proofs covering one claim are two things that must
/// hold, not a menu.
pub fn attest(record: &Record, bindings: &[String]) -> Attested {
    if bindings.is_empty() {
        return Attested::Unbound;
    }
    for binding in bindings {
        if let Some(skipped) = record.skipped.iter().find(|s| &s.path == binding) {
            return Attested::NoEvidence {
                path: binding.clone(),
                why: skipped.reason.clone(),
            };
        }
        if record.deselected.iter().any(|d| d == binding) {
            return Attested::NoEvidence {
                path: binding.clone(),
                why: "deselected by the run's selection".to_string(),
            };
        }
        match record.executed.get(binding) {
            Some(Executed::Passed) => {}
            Some(&outcome) => {
                return Attested::Red {
                    path: binding.clone(),
                    outcome,
                }
            }
            // Absent from a record that names its skips and deselections: this proof was not in the
            // run at all. A record from before the proof was written looks exactly like this, which
            // is the correct reading — that run is not evidence for it.
            None => {
                return Attested::NoEvidence {
                    path: binding.clone(),
                    why: "not present in the recorded run".to_string(),
                }
            }
        }
    }
    Attested::Yes {
        path: bindings[0].clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_with(executed: &[(&str, Executed)], skipped: &[(&str, &str)], desel: &[&str]) -> Record {
        Record {
            schema: 1,
            version: "test".into(),
            binary: "test".into(),
            selection: Vec::new(),
            duration_ms: 0,
            summary: Counts::default(),
            executed: executed.iter().map(|(p, o)| (p.to_string(), *o)).collect(),
            skipped: skipped
                .iter()
                .map(|(p, r)| Skipped { path: p.to_string(), reason: r.to_string() })
                .collect(),
            deselected: desel.iter().map(|d| d.to_string()).collect(),
            reminders: Vec::new(),
            deputed: Vec::new(),
            measurements: Vec::new(),
            attached: Vec::new(),
        }
    }

    /// The one case that matters: a green suite, a real binding, and no evidence — because the
    /// covering proof never ran. This is the shape `0 failed` hides.
    #[test]
    fn a_skipped_proof_attests_nothing_and_carries_its_reason() {
        let r = record_with(&[], &[("drain", "requires \"broker\" (unavailable)")], &[]);
        let verdict = attest(&r, &["drain".to_string()]);
        assert!(!verdict.is_attested());
        match verdict {
            Attested::NoEvidence { why, .. } => assert!(why.contains("broker"), "reason: {why}"),
            other => panic!("expected NoEvidence, got {other:?}"),
        }
    }

    /// Deselection and absence are different causes with one consequence, and both must refuse.
    #[test]
    fn deselected_and_absent_proofs_attest_nothing() {
        let desel = record_with(&[], &[], &["busy"]);
        assert!(!attest(&desel, &["busy".to_string()]).is_attested());

        let absent = record_with(&[("other", Executed::Passed)], &[], &[]);
        assert!(!attest(&absent, &["busy".to_string()]).is_attested());
    }

    /// An open promise is red by definition — a proof authored ahead of its implementation is the
    /// opposite of evidence, and must never attest.
    #[test]
    fn an_open_spec_never_attests() {
        let r = record_with(&[("drain", Executed::Spec)], &[], &[]);
        assert!(!attest(&r, &["drain".to_string()]).is_attested());
    }

    /// An address nothing covers is unbound — reported as such, and never as a pass.
    #[test]
    fn an_unbound_address_is_not_a_pass() {
        let r = record_with(&[("busy", Executed::Passed)], &[], &[]);
        assert_eq!(attest(&r, &[]), Attested::Unbound);
        assert!(!Attested::Unbound.is_attested());
    }

    /// Two proofs covering one claim are two things that must hold, not a menu — so one passing
    /// sibling cannot carry a skipped one.
    #[test]
    fn every_binding_must_produce_evidence_not_just_one() {
        let r = record_with(&[("a", Executed::Passed)], &[("b", "no docker")], &[]);
        assert!(!attest(&r, &["a".to_string(), "b".to_string()]).is_attested());
    }

    /// The honest case still passes, or the atom is unusable.
    #[test]
    fn an_executed_passing_proof_attests() {
        let r = record_with(&[("busy", Executed::Passed)], &[], &[]);
        assert!(attest(&r, &["busy".to_string()]).is_attested());
    }
}
