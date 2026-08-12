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
    /// Declared run-to-run noise: the gate holds `value - tolerance` (mirrored for
    /// lower-is-better) while banking still records the best-seen value — a noisy metric
    /// (black-box coverage wobbles with timing-dependent paths) declares its band instead of
    /// flaking or being hand-loosened after every bank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
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

/// Every banked metric across every set, flattened to name → value — the reminder-condition
/// view (`account.baselines`), where a drift policy compares against what was deliberately
/// banked (docs/design/reminders.md#duration-drift-is-attention). Set files are read in name
/// order; metric names are unique by convention, so collisions are theoretical (last wins).
pub fn load_all(root: &Path) -> Vec<(String, f64)> {
    let mut out = std::collections::BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir(root)) {
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for file in files {
            if file.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(set) = file.file_stem().and_then(|s| s.to_str()) else { continue };
            for (name, metric) in load(root, set).metrics {
                out.insert(name, metric.value);
            }
        }
    }
    out.into_iter().collect()
}

fn store(root: &Path, set: &str, base: &Baselines) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(root))?;
    let text = serde_json::to_string_pretty(base).unwrap_or_default();
    std::fs::write(path(root, set), text + "\n")
}

/// Which metrics a banking pass may move — the steady-state policy
/// (docs/design/verifiers.md#baseline-bank-policy).
#[derive(Debug, Clone, PartialEq)]
pub enum BankSelection {
    /// Bare `--update-baseline`: establish first-sights, tighten ONLY goal-carrying metrics
    /// (active debt). A goal-less metric is a protection whose committed floor never moves
    /// without a hand — its improvements stay green and unbanked (steady-state slack).
    GoalCarrying,
    /// `--update-baseline=<sel,…>`: move exactly the matching metrics (each selector is a
    /// substring over the metric name — the `--heed=SEL` spelling family), goal or no goal.
    /// A selector that matches nothing is reported loudly, never a silent no-op.
    Named(Vec<String>),
}

impl BankSelection {
    fn covers(&self, name: &str, existing: &Metric) -> bool {
        match self {
            BankSelection::GoalCarrying => existing.goal.is_some(),
            BankSelection::Named(sels) => sels.iter().any(|s| name.contains(s.as_str())),
        }
    }
}

/// What an [`update`] pass did, for the caller to report.
#[derive(Debug, Default)]
pub struct UpdateReport {
    pub established: Vec<String>,
    pub tightened: Vec<String>,
    pub refused: Vec<String>,
    /// Improvements a goal-less metric measured but did not bank — steady-state slack, named so
    /// the human can bank deliberately (`--update-baseline=<name>`) instead of wondering.
    pub held: Vec<String>,
}

/// Update baselines under `root` from this run's measurements, grouped by set, moving only what
/// `selection` covers. First sight of a metric always establishes (a metric with no floor gates
/// nothing). A covered metric tightens on improvement and REFUSES to loosen — the guard is
/// absolute on every flag path; loosening is a hand edit reviewed in the diff. An uncovered
/// metric holds: its improvement is reported, not written.
pub fn update(root: &Path, measurements: &[Measurement], selection: &BankSelection) -> UpdateReport {
    let mut report = UpdateReport::default();
    let mut matched: Vec<bool> = match selection {
        BankSelection::Named(sels) => vec![false; sels.len()],
        BankSelection::GoalCarrying => Vec::new(),
    };
    let mut by_set: BTreeMap<String, Vec<&Measurement>> = BTreeMap::new();
    for m in measurements {
        by_set.entry(m.set.clone()).or_default().push(m);
    }
    for (set, ms) in by_set {
        let mut base = load(root, &set);
        base.schema = SCHEMA;
        for m in ms {
            if let BankSelection::Named(sels) = selection {
                for (i, s) in sels.iter().enumerate() {
                    if m.name.contains(s.as_str()) {
                        matched[i] = true;
                    }
                }
            }
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
                            tolerance: None,
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
                    if !selection.covers(&m.name, existing) {
                        // Steady-state: not this pass's to move. A strict improvement is worth a
                        // line; an equal or regressed value is the gate's business, not the bank's.
                        if improves && m.value != existing.value {
                            report.held.push(format!(
                                "{set}:{} stays at {} (measured {}; no goal — bank it by name: --update-baseline={})",
                                m.name, existing.value, m.value, m.name
                            ));
                        }
                        continue;
                    }
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
    if let BankSelection::Named(sels) = selection {
        for (s, hit) in sels.iter().zip(matched) {
            if !hit {
                report.refused.push(format!(
                    "--update-baseline={s}: no recorded measurement matches {s:?} (a typo never silently no-ops)"
                ));
            }
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
        for line in &self.held {
            eprintln!("prova: baseline held {line}");
        }
        for line in &self.refused {
            eprintln!("prova: baseline REFUSED {line}");
        }
        if self.established.is_empty()
            && self.tightened.is_empty()
            && self.held.is_empty()
            && self.refused.is_empty()
        {
            eprintln!("prova: --update-baseline: no measurements recorded this run");
        }
        self.refused.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::make_tempdir;

    /// Hand-edit a goal into a committed metric, the way a human schedules a paydown.
    fn schedule_goal(root: &std::path::Path, set: &str, name: &str, goal: f64) {
        let p = root.join(format!(".prova/baselines/{set}.json"));
        let mut base: Baselines =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        base.metrics.get_mut(name).unwrap().goal = Some(goal);
        std::fs::write(&p, serde_json::to_string(&base).unwrap()).unwrap();
    }

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
        let report = update(
            &root,
            &[m("rust.unwraps", 20.0, Direction::LowerIsBetter)],
            &BankSelection::GoalCarrying,
        );
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
        update(&root, &[m("clones", 10.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        schedule_goal(&root, "quality", "clones", 0.0);

        let report = update(&root, &[m("clones", 8.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        assert_eq!(report.tightened.len(), 1);
        assert_eq!(load(&root, "quality").metrics["clones"].value, 8.0);

        let report = update(&root, &[m("clones", 12.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        assert_eq!(report.refused.len(), 1);
        assert!(report.tightened.is_empty());
        assert_eq!(load(&root, "quality").metrics["clones"].value, 8.0);
    }

    /// The same guard mirrored for higher_is_better: coverage climbing banks, slipping is refused.
    #[test]
    fn update_tightens_and_refuses_higher_is_better() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("coverage", 60.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);
        schedule_goal(&root, "quality", "coverage", 100.0);

        let report = update(&root, &[m("coverage", 70.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);
        assert_eq!(report.tightened.len(), 1);
        assert_eq!(load(&root, "quality").metrics["coverage"].value, 70.0);

        let report = update(&root, &[m("coverage", 65.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);
        assert_eq!(report.refused.len(), 1);
        assert_eq!(load(&root, "quality").metrics["coverage"].value, 70.0);
    }

    /// An equal value counts as an improvement (`<=` / `>=`), so re-banking an unchanged metric
    /// is a tighten, not a refusal — what keeps `--update-baseline` idempotent.
    #[test]
    fn update_equal_value_rebanks() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("clones", 5.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        schedule_goal(&root, "quality", "clones", 0.0);
        let report = update(&root, &[m("clones", 5.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        assert_eq!(report.tightened.len(), 1);
        assert!(report.refused.is_empty() && report.held.is_empty(), "equal is a rebank, never held noise");
    }

    /// The paydown fields survive a tightening update untouched — the goal machinery depends on
    /// this: banking a gain must never silently retire the goal that demanded it.
    #[test]
    fn update_carries_goal_through_tighten() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("expects", 25.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        // Hand-edit the file the way a human schedules a paydown.
        let p = root.join(".prova/baselines/quality.json");
        let mut base: Baselines = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let metric = base.metrics.get_mut("expects").unwrap();
        metric.goal = Some(10.0);
        metric.deadline = Some("2026-12-31".to_string());
        std::fs::write(&p, serde_json::to_string(&base).unwrap()).unwrap();

        update(&root, &[m("expects", 20.0, Direction::LowerIsBetter)], &BankSelection::GoalCarrying);
        let metric = &load(&root, "quality").metrics["expects"];
        assert_eq!(metric.value, 20.0);
        assert_eq!(metric.goal, Some(10.0));
        assert_eq!(metric.deadline.as_deref(), Some("2026-12-31"));
    }

    /// A declared `tolerance` survives a tightening bank exactly like a goal — re-peaking the
    /// floor must never silently drop the noise band that keeps it from flaking.
    #[test]
    fn update_carries_tolerance_through_tighten() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("bb", 60.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);
        let p = root.join(".prova/baselines/quality.json");
        let mut base: Baselines = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        base.metrics.get_mut("bb").unwrap().tolerance = Some(1.0);
        std::fs::write(&p, serde_json::to_string(&base).unwrap()).unwrap();

        update(&root, &[m("bb", 62.0, Direction::HigherIsBetter)], &BankSelection::Named(vec!["bb".into()]));
        let metric = &load(&root, "quality").metrics["bb"];
        assert_eq!(metric.value, 62.0);
        assert_eq!(metric.tolerance, Some(1.0));
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
        update(&root, &[a, b], &BankSelection::GoalCarrying);
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

    /// The steady-state policy: a goal-less metric's improvement is HELD (green, unbanked,
    /// named in the report with the named-banking spelling) — a lucky run never mints a floor
    /// nobody chose. The refuse-to-loosen guard needs no bank coverage: the gate owns regressions.
    #[test]
    fn bare_update_holds_goalless_improvements() {
        let root = make_tempdir().unwrap();
        update(&root, &[m("coverage", 60.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);

        let report = update(&root, &[m("coverage", 75.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);
        assert!(report.tightened.is_empty(), "no goal, no tighten");
        assert_eq!(report.held.len(), 1);
        assert!(report.held[0].contains("--update-baseline=coverage"), "{:?}", report.held);
        assert_eq!(load(&root, "quality").metrics["coverage"].value, 60.0, "the floor did not move");

        // A goal-less regression is the ratchet gate's business — the bank stays silent.
        let report = update(&root, &[m("coverage", 50.0, Direction::HigherIsBetter)], &BankSelection::GoalCarrying);
        assert!(report.held.is_empty() && report.refused.is_empty() && report.tightened.is_empty());
    }

    /// Named banking moves exactly what was asked for — goal or no goal — and the loosen guard
    /// stays absolute even when the metric is named.
    #[test]
    fn named_banking_moves_exactly_the_named_metrics() {
        let root = make_tempdir().unwrap();
        update(
            &root,
            &[
                m("coverage.unit", 60.0, Direction::HigherIsBetter),
                m("coverage.lines", 80.0, Direction::HigherIsBetter),
            ],
            &BankSelection::GoalCarrying,
        );

        let sel = BankSelection::Named(vec!["coverage.unit".into()]);
        let report = update(
            &root,
            &[
                m("coverage.unit", 65.0, Direction::HigherIsBetter),
                m("coverage.lines", 85.0, Direction::HigherIsBetter),
            ],
            &sel,
        );
        assert_eq!(report.tightened.len(), 1, "{report:?}");
        assert_eq!(load(&root, "quality").metrics["coverage.unit"].value, 65.0);
        assert_eq!(load(&root, "quality").metrics["coverage.lines"].value, 80.0, "unnamed holds");

        let report = update(&root, &[m("coverage.unit", 55.0, Direction::HigherIsBetter)], &sel);
        assert_eq!(report.refused.len(), 1, "naming a metric never authorizes loosening it");
        assert_eq!(load(&root, "quality").metrics["coverage.unit"].value, 65.0);
    }

    /// A selector that matches no recorded measurement is a loud refusal — a typo must never
    /// read as a successful no-op bank.
    #[test]
    fn named_banking_reports_selectors_that_matched_nothing() {
        let root = make_tempdir().unwrap();
        let report = update(
            &root,
            &[m("coverage.unit", 60.0, Direction::HigherIsBetter)],
            &BankSelection::Named(vec!["covrage".into()]),
        );
        assert_eq!(report.refused.len(), 1);
        assert!(report.refused[0].contains("covrage"), "{:?}", report.refused);
        assert!(!report.print(), "an unmatched selector flips the verdict");
    }
}
