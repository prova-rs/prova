//! Ratchet baselines — the committed floor a `measure.ratchet` gate holds the line against, and the
//! one guarded path that may move it.
//!
//! Stored per set at `<root>/.prova/baselines/<set>.json` — the TRACKED half of `.prova/` (unlike
//! `var/`, which is git-ignored per-run state), so a baseline is a reviewable fact that travels with
//! the code. Keys are namespaced (`rust.file.engine_rs.lines`, `coverage.rust.line`); each metric
//! carries its own direction, so one file mixes languages and directions freely, and the per-set
//! split is purely for ownership/merge-conflict isolation in a polyglot repo.
//!
//! The guard: `prova --update-baseline` tightens freely (an improvement, or the first sight of a
//! metric) and REFUSES to loosen (a regression). Loosening is intentionally not a flag — it is a
//! hand-edit of the committed file, which shows up in review. The agent is on the other side of this
//! wall, which is why it lives in the binary and not in a Lua recipe the agent could rewrite.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::home::Home;

/// Schema version of a baseline file, so a future reader can refuse an unfamiliar shape.
pub const SCHEMA: u32 = 1;

/// One baseline set (`.prova/baselines/<set>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baselines {
    pub schema: u32,
    pub metrics: BTreeMap<String, Metric>,
}

impl Default for Baselines {
    fn default() -> Self {
        Baselines {
            schema: SCHEMA,
            metrics: BTreeMap::new(),
        }
    }
}

/// One metric's committed floor/ceiling and which way is better.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub value: f64,
    /// `"lower_is_better"` | `"higher_is_better"` — matches [`crate::record::direction_str`].
    pub direction: String,
    // Forward-compat for paydown (PR5): a target beyond `value`, an optional per-window pace, and a
    // deadline. Carried through updates untouched; the flat ratchet ignores them today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
}

fn dir(home: &Home) -> PathBuf {
    home.dir.join(".prova").join("baselines")
}

fn path(home: &Home, set: &str) -> PathBuf {
    dir(home).join(format!("{set}.json"))
}

/// Read a baseline set, or an empty one if the file is absent/unparseable (a missing floor is not an
/// error here — the ratchet gate treats "no baseline for this metric" as its own loud failure).
pub fn load(home: &Home, set: &str) -> Baselines {
    std::fs::read_to_string(path(home, set))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn store(home: &Home, set: &str, base: &Baselines) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(home))?;
    let text = serde_json::to_string_pretty(base).unwrap_or_default();
    std::fs::write(path(home, set), text + "\n")
}

/// What an `--update-baseline` pass did, for the human running it.
#[derive(Debug, Default)]
pub struct UpdateReport {
    pub established: Vec<String>,
    pub tightened: Vec<String>,
    pub refused: Vec<String>,
}

/// Update baselines from this run's measurements, grouped by set. Tightens freely (improvement or
/// first sight); refuses to loosen (regression), leaving that metric's committed floor intact. The
/// refused ones are reported, not written — the guard.
pub fn update(home: &Home, measurements: &[prova_core::Measurement]) -> UpdateReport {
    let mut report = UpdateReport::default();
    let mut by_set: BTreeMap<String, Vec<&prova_core::Measurement>> = BTreeMap::new();
    for m in measurements {
        by_set.entry(m.set.clone()).or_default().push(m);
    }
    for (set, ms) in by_set {
        let mut base = load(home, &set);
        base.schema = SCHEMA;
        for m in ms {
            let dir_str = crate::record::direction_str(m.direction).to_string();
            match base.metrics.get(&m.name) {
                None => {
                    base.metrics.insert(
                        m.name.clone(),
                        Metric {
                            value: m.value,
                            direction: dir_str,
                            goal: None,
                            pace: None,
                            deadline: None,
                        },
                    );
                    report
                        .established
                        .push(format!("{set}:{} = {}", m.name, m.value));
                }
                Some(existing) => {
                    let improves = match m.direction {
                        prova_core::Direction::LowerIsBetter => m.value <= existing.value,
                        prova_core::Direction::HigherIsBetter => m.value >= existing.value,
                    };
                    if improves {
                        let mut updated = existing.clone();
                        let from = updated.value;
                        updated.value = m.value;
                        updated.direction = dir_str;
                        base.metrics.insert(m.name.clone(), updated);
                        report
                            .tightened
                            .push(format!("{set}:{} {from} -> {}", m.name, m.value));
                    } else {
                        report.refused.push(format!(
                            "{set}:{} would loosen {} -> {} (kept; hand-edit the file to loosen intentionally)",
                            m.name, existing.value, m.value
                        ));
                    }
                }
            }
        }
        if let Err(e) = store(home, &set, &base) {
            report
                .refused
                .push(format!("{set}: could not write baseline: {e}"));
        }
    }
    report
}

impl UpdateReport {
    /// Print the outcome and return whether every measurement was accepted (nothing refused).
    pub fn print(&self) -> bool {
        for line in &self.established {
            eprintln!("prova: baseline established {line}");
        }
        for line in &self.tightened {
            eprintln!("prova: baseline tightened {line}");
        }
        for line in &self.refused {
            eprintln!("prova: baseline REFUSED {line}");
        }
        if self.established.is_empty() && self.tightened.is_empty() && self.refused.is_empty() {
            eprintln!("prova: --update-baseline: no measurements recorded this run");
        }
        self.refused.is_empty()
    }
}
