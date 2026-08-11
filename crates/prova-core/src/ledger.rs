//! The run record — what a run actually executed, and what it did not.
//!
//! This is the library-side, path-injected version of the run record. Callers supply the path to
//! the record they want to read; this module does not know about project roots, `Home`, or the
//! CLI's `var/` layout.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What became of one leaf that actually ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Executed {
    Passed,
    Failed,
    /// An open promise: red by definition, and therefore not evidence of anything working.
    /// Serializes as `"promised"`; `alias = "spec"` still reads a pre-rename (schema 1) record so a
    /// stale run record loads rather than erroring until the next run rewrites it.
    #[serde(alias = "spec")]
    Promised,
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
/// Every field is defaulted on read: a record written before a count existed (schema 1 predates
/// `promised`) must LOAD — the same stale-record tolerance as `Executed`'s `spec` alias, and
/// without it that alias is dead code: the summary's parse failure silently reads as "no run
/// recorded" before the alias ever gets its chance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Counts {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub promised: usize,
    pub deselected: usize,
}

/// One reminder, as the run evaluated it — the attention account's row in the record.
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
    /// Free-form tags (same grammar as tests), so a context can heed a subset of DUE reminders by
    /// name or tag. Defaulted so records from before tags existed still parse.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
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

/// One deputed case — a verdict another verifier produced, conducted by a proof and adopted
/// into this run's account.
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


/// One run, as a durable fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Schema version of this file, so a future reader can refuse an unfamiliar shape rather than
    /// silently misread it.
    pub schema: u32,
    /// The prova version that produced it.
    pub version: String,
    /// Which build of the binary produced it.
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

    /// The attention account: every reminder with its evaluated state.
    #[serde(default)]
    pub reminders: Vec<ReminderEntry>,
    /// The deputed account: every case a verifier facet ingested this run, with provenance.
    #[serde(default)]
    pub deputed: Vec<DeputedRow>,
}

pub mod claims;

pub use claims::{
    Claim, ClaimError, Kind, Owed, Status, backlog, digest, matching_id, pin, promote, reconcile,
    scan, split_pin,
};

/// Read the record at the given path.
pub fn read_record(path: &Path) -> Result<Record, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse {}: {e}", path.display()))
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
/// The whole account, computed once — every stage of the lifecycle with its count, plus the
/// debts. The stages, statically: a claim is BOUND when at least one proof covers it. PROMISED is
/// the open surface across every origin (claim-covering or not — an open promise is owed either
/// way). ATTESTED needs the record, and its absence is `None` — a stated fact, never a zero.
#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub claimed: usize,
    pub bound: usize,
    pub promised: usize,
    pub attested: Option<usize>,
    pub owed: Vec<Owed>,
}

/// Reconcile claims, obligations and an optional run record into the whole account.
pub fn account(
    claims: &[Claim],
    proofs: &[crate::ProofObligation],
    record: Option<&Record>,
) -> Account {
    let bindings_for = |address: &str| -> Vec<String> {
        proofs
            .iter()
            .filter(|p| p.covers.iter().any(|c| split_pin(c).0 == address))
            .map(|p| p.path.clone())
            .collect()
    };
    // The account speaks only of claims. Backlog items are the muted state — captured, but not part
    // of what is CLAIMED/BOUND/ATTESTED — so they are filtered out here exactly as they are from
    // `owed`. `reconcile` still sees them all: a proof covering a cold item is worth a BACKLOGGED row.
    let active = || claims.iter().filter(|c| c.kind == Kind::Claim);
    let claimed = active().count();
    let bound = active().filter(|c| !bindings_for(&c.address).is_empty()).count();
    let promised = proofs.iter().filter(|p| p.promises.is_some()).count();
    let attested = record.as_ref().map(|r| {
        active()
            .filter(|c| attest(r, &bindings_for(&c.address)).is_attested())
            .count()
    });
    let owed = reconcile(claims, proofs);
    Account {
        claimed,
        bound,
        promised,
        attested,
        owed,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The schema-1 back-compat contract: `"promised"` is the current spelling, and a pre-rename
    /// record's `"spec"` still loads (until the next run rewrites it) — a stale record must load,
    /// never error. Unknown spellings refuse rather than silently misread.
    #[test]
    fn executed_reads_both_spellings_and_refuses_unknown() {
        assert_eq!(serde_json::to_string(&Executed::Promised).unwrap(), "\"promised\"");
        assert_eq!(serde_json::from_str::<Executed>("\"promised\"").unwrap(), Executed::Promised);
        assert_eq!(serde_json::from_str::<Executed>("\"spec\"").unwrap(), Executed::Promised);
        assert_eq!(serde_json::from_str::<Executed>("\"passed\"").unwrap(), Executed::Passed);
        assert!(serde_json::from_str::<Executed>("\"exploded\"").is_err());
    }

    /// A record from before `measurements`/`deputed`/`reminders` existed still parses — the
    /// defaulted fields are what let an old var/ record survive an upgrade.
    #[test]
    fn a_minimal_old_record_still_parses() {
        let old = r#"{
            "schema": 1, "version": "0.11.0", "binary": "x", "selection": [],
            "duration_ms": 5, "summary": {},
            "executed": { "a › b": "spec" }, "skipped": [], "deselected": []
        }"#;
        let rec: Record = serde_json::from_str(old).unwrap();
        assert_eq!(rec.executed["a › b"], Executed::Promised);
        assert!(rec.measurements.is_empty());
    }

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

    #[test]
    fn deselected_and_absent_proofs_attest_nothing() {
        let desel = record_with(&[], &[], &["busy"]);
        assert!(!attest(&desel, &["busy".to_string()]).is_attested());

        let absent = record_with(&[("other", Executed::Passed)], &[], &[]);
        assert!(!attest(&absent, &["busy".to_string()]).is_attested());
    }

    #[test]
    fn an_open_spec_never_attests() {
        let r = record_with(&[("drain", Executed::Promised)], &[], &[]);
        assert!(!attest(&r, &["drain".to_string()]).is_attested());
    }

    #[test]
    fn an_unbound_address_is_not_a_pass() {
        let r = record_with(&[("busy", Executed::Passed)], &[], &[]);
        assert_eq!(attest(&r, &[]), Attested::Unbound);
        assert!(!Attested::Unbound.is_attested());
    }

    #[test]
    fn every_binding_must_produce_evidence_not_just_one() {
        let r = record_with(&[("a", Executed::Passed)], &[("b", "no docker")], &[]);
        assert!(!attest(&r, &["a".to_string(), "b".to_string()]).is_attested());
    }

    #[test]
    fn an_executed_passing_proof_attests() {
        let r = record_with(&[("busy", Executed::Passed)], &[], &[]);
        assert!(attest(&r, &["busy".to_string()]).is_attested());
    }
}
