//! Ratchet baselines — the committed floor a `measure.ratchet` gate holds the line against, and the
//! one guarded path that may move it.
//!
//! Stored per set at `<root>/.prova/baselines/<set>.json` — the TRACKED half of `.prova/` (unlike
//! `var/`, which is git-ignored per-run state), so a baseline is a reviewable fact that travels with
//! the code. Keys are namespaced (`rust.file.engine_rs.lines`, `coverage.rust.line`); each metric
//! carries its own direction, so one file mixes languages and directions freely, and the per-set
//! split is purely for ownership/merge-conflict isolation in a polyglot repo.
//!
//! The guard: [`update`] tightens freely (an improvement, or the first sight of a metric) and REFUSES
//! to loosen (a regression). Loosening is intentionally not an option here — it is a hand-edit of the
//! committed file, which shows up in review. The agent is on the other side of this wall, which is why
//! the writer lives in the engine (reachable by an embedder) and not in a Lua recipe the agent could
//! rewrite. The READ half is the `measure.ratchet` recipe; this is the WRITE half.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{Direction, Measurement};

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
    /// `"lower_is_better"` | `"higher_is_better"` — matches [`Direction::as_str`].
    pub direction: String,
    // Paydown (PR5): a target beyond `value`, an optional per-window pace, and a deadline. The
    // `measure.ratchet` recipe reads `goal`/`deadline`; `pace` is reserved. Carried through updates
    // untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
}

fn dir(root: &Path) -> PathBuf {
    root.join(".prova").join("baselines")
}

fn path(root: &Path, set: &str) -> PathBuf {
    dir(root).join(format!("{set}.json"))
}

/// Read a baseline set, or an empty one if the file is absent/unparseable (a missing floor is not an
/// error here — the ratchet gate treats "no baseline for this metric" as its own loud failure).
pub fn load(root: &Path, set: &str) -> Baselines {
    std::fs::read_to_string(path(root, set))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn store(root: &Path, set: &str, base: &Baselines) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(root))?;
    let text = serde_json::to_string_pretty(base).unwrap_or_default();
    std::fs::write(path(root, set), text + "\n")
}

/// What an [`update`] pass did, for the caller to report.
#[derive(Debug, Default)]
pub struct UpdateReport {
    pub established: Vec<String>,
    pub tightened: Vec<String>,
    pub refused: Vec<String>,
}

/// Update baselines under `root` from this run's measurements, grouped by set. Tightens freely
/// (improvement or first sight); refuses to loosen (regression), leaving that metric's committed
/// floor intact. The refused ones are reported, not written — the guard.
pub fn update(root: &Path, measurements: &[Measurement]) -> UpdateReport {
    let mut report = UpdateReport::default();
    let mut by_set: BTreeMap<String, Vec<&Measurement>> = BTreeMap::new();
    for m in measurements {
        by_set.entry(m.set.clone()).or_default().push(m);
    }
    for (set, ms) in by_set {
        let mut base = load(root, &set);
        base.schema = SCHEMA;
        for m in ms {
            let dir_str = m.direction.as_str().to_string();
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
                        Direction::LowerIsBetter => m.value <= existing.value,
                        Direction::HigherIsBetter => m.value >= existing.value,
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
        if let Err(e) = store(root, &set, &base) {
            report
                .refused
                .push(format!("{set}: could not write baseline: {e}"));
        }
    }
    report
}

impl UpdateReport {
    /// Print the outcome to stderr and return whether every measurement was accepted (nothing
    /// refused). A convenience for CLI/embedder callers; the fields are public for other renderings.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::make_tempdir;

    fn m(name: &str, value: f64, direction: Direction) -> Measurement {
        Measurement {
            name: name.to_string(),
            value,
            direction,
            set: "quality".to_string(),
        }
    }

    /// A missing file is an empty set, not an error — "no baseline" is the ratchet GATE's loud
    /// failure, never the loader's.
    #[test]
    fn load_absent_is_empty() {
        let root = make_tempdir().unwrap();
        let base = load(&root, "quality");
        assert_eq!(base.schema, SCHEMA);
        assert!(base.metrics.is_empty());
    }

    /// An unparseable file also loads empty rather than erroring — same contract as absent.
    #[test]
    fn load_garbage_is_empty() {
        let root = make_tempdir().unwrap();
        std::fs::create_dir_all(root.join(".prova/baselines")).unwrap();
        std::fs::write(root.join(".prova/baselines/quality.json"), "not json {").unwrap();
        assert!(load(&root, "quality").metrics.is_empty());
    }

    /// First sight of a metric establishes it: value AND direction land in the file, and the
    /// report says `established`, not `tightened`.
    #[test]
    fn update_establishes_first_sight() {
        let root = make_tempdir().unwrap();
        let report = update(&root, &[m("rust.unwraps", 20.0, Direction::LowerIsBetter)]);
        assert_eq!(report.established.len(), 1);
        assert!(report.tightened.is_empty() && report.refused.is_empty());
        let base = load(&root, "quality");
        let metric = &base.metrics["rust.unwraps"];
        assert_eq!(metric.value, 20.0);
        assert_eq!(metric.direction, "lower_is_better");
    }

    /// The guard, both directions: an improvement tightens the committed value; a regression is
    /// REFUSED and the committed value stays exactly where it was.
    #[test]
    fn update_tightens_and_refuses_lower_is_better() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("clones", 10.0, Direction::LowerIsBetter)]);

        let report = update(&root, &[m("clones", 8.0, Direction::LowerIsBetter)]);
        assert_eq!(report.tightened.len(), 1);
        assert_eq!(load(&root, "quality").metrics["clones"].value, 8.0);

        let report = update(&root, &[m("clones", 12.0, Direction::LowerIsBetter)]);
        assert_eq!(report.refused.len(), 1);
        assert!(report.tightened.is_empty());
        assert_eq!(load(&root, "quality").metrics["clones"].value, 8.0);
    }

    /// The same guard mirrored for higher_is_better: coverage climbing banks, slipping is refused.
    #[test]
    fn update_tightens_and_refuses_higher_is_better() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("coverage", 60.0, Direction::HigherIsBetter)]);

        let report = update(&root, &[m("coverage", 70.0, Direction::HigherIsBetter)]);
        assert_eq!(report.tightened.len(), 1);
        assert_eq!(load(&root, "quality").metrics["coverage"].value, 70.0);

        let report = update(&root, &[m("coverage", 65.0, Direction::HigherIsBetter)]);
        assert_eq!(report.refused.len(), 1);
        assert_eq!(load(&root, "quality").metrics["coverage"].value, 70.0);
    }

    /// An equal value counts as an improvement (`<=` / `>=`), so re-banking an unchanged metric
    /// is a tighten, not a refusal — what keeps `--update-baseline` idempotent.
    #[test]
    fn update_equal_value_rebanks() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("clones", 5.0, Direction::LowerIsBetter)]);
        let report = update(&root, &[m("clones", 5.0, Direction::LowerIsBetter)]);
        assert_eq!(report.tightened.len(), 1);
        assert!(report.refused.is_empty());
    }

    /// The paydown fields survive a tightening update untouched — the goal machinery depends on
    /// this: banking a gain must never silently retire the goal that demanded it.
    #[test]
    fn update_carries_goal_through_tighten() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("expects", 25.0, Direction::LowerIsBetter)]);
        // Hand-edit the file the way a human schedules a paydown.
        let p = root.join(".prova/baselines/quality.json");
        let mut base: Baselines = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let metric = base.metrics.get_mut("expects").unwrap();
        metric.goal = Some(10.0);
        metric.deadline = Some("2026-12-31".to_string());
        std::fs::write(&p, serde_json::to_string(&base).unwrap()).unwrap();

        update(&root, &[m("expects", 20.0, Direction::LowerIsBetter)]);
        let metric = &load(&root, "quality").metrics["expects"];
        assert_eq!(metric.value, 20.0);
        assert_eq!(metric.goal, Some(10.0));
        assert_eq!(metric.deadline.as_deref(), Some("2026-12-31"));
    }

    /// Sets partition into separate files — the ownership/merge-conflict isolation the per-set
    /// split exists for.
    #[test]
    fn update_splits_sets_into_files() {
        let root = make_tempdir().unwrap();
        let mut a = m("x", 1.0, Direction::LowerIsBetter);
        a.set = "alpha".to_string();
        let mut b = m("y", 2.0, Direction::LowerIsBetter);
        b.set = "beta".to_string();
        update(&root, &[a, b]);
        assert!(root.join(".prova/baselines/alpha.json").is_file());
        assert!(root.join(".prova/baselines/beta.json").is_file());
        assert_eq!(load(&root, "alpha").metrics["x"].value, 1.0);
        assert_eq!(load(&root, "beta").metrics["y"].value, 2.0);
    }

    /// The report's verdict: refusals flip `print`'s bool — the exit-code seam an embedder gates on.
    #[test]
    fn report_verdict_tracks_refusals() {
        assert!(UpdateReport::default().print());
        let refused = UpdateReport {
            refused: vec!["x".to_string()],
            ..Default::default()
        };
        assert!(!refused.print());
    }
}
