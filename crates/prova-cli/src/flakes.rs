//! The flake ledger's CLI half — detecting a flake while the run streams, and the account it leaves.
//!
//! THE RULE (prova_core::ledger::FlakeEvent): a test is FLAKY when a failure and a pass are recorded
//! for it on the same tree digest and the same prova binary — the `--resume` soundness key. A flake
//! is a concurrency defect, never "load", and it must never read as green.
//!
//! The run's own history is what makes the rule checkable: before this run's journal replaces the
//! previous one, the previous journal of the same lane — when its tree and binary match — lends
//! its failures (`History::leaves`), and the previous `last-run.json` on the same tree lends its
//! deputed failures (`History::deputed`). A leaf that passes here but failed there is `flaky`; so is
//! a deputed case, and so is any case the deputy itself marked flaky (a retry, or a `junit.merge`
//! that replaced a red with a re-run pass).

use std::collections::BTreeMap;

use prova_core::ledger::{DeputedRef, FlakeEvent, FLAKE_SCHEMA};

/// A deputed case's identity: verifier, suite, name.
pub(crate) type CaseKey = (String, String, String);

/// What earlier runs on this tree and binary recorded as failing: key → (the run that failed it, its
/// first failure line).
#[derive(Debug, Default, Clone)]
pub(crate) struct History {
    pub leaves: BTreeMap<String, (String, Option<String>)>,
    pub deputed: BTreeMap<CaseKey, (String, Option<String>)>,
}

impl History {
    /// The deputed failures a prior record lends — only when it was recorded over `tree` by `binary`.
    pub(crate) fn deputed_from(record: &prova_core::ledger::Record, tree: &str, binary: &str) -> BTreeMap<CaseKey, (String, Option<String>)> {
        let mut out = BTreeMap::new();
        if record.tree.as_deref() != Some(tree) || record.binary != binary {
            return out;
        }
        for d in &record.deputed {
            let from = match d.outcome.as_str() {
                "failed" | "error" => d.reused_from.clone().unwrap_or_else(|| record.run_id.clone()),
                "flaky" => d.flaky_from.clone().unwrap_or_else(|| record.run_id.clone()),
                _ => continue,
            };
            out.insert((d.verifier.clone(), d.suite.clone(), d.name.clone()), (from, d.message.clone()));
        }
        out
    }
}

/// The flake check for one run: the history it judges against, and the events it has recorded.
pub(crate) struct Watch {
    history: History,
    pub(crate) run_id: String,
    tree: Option<String>,
    binary: String,
    /// `"resume"` under `--resume`, else `"rerun"` — how a leaf's earlier failure reached this run.
    leaf_source: &'static str,
    deputed: Option<prova_core::DeputedRegistry>,
    pub(crate) events: Vec<FlakeEvent>,
    /// Set once the deputed account has been judged, so the run's end is judged exactly once.
    settled: bool,
}

/// The first line of a failure message — a label for the ledger, not a traceback.
pub(crate) fn first_line(message: Option<&str>) -> Option<String> {
    message.and_then(|m| m.lines().map(str::trim).find(|l| !l.is_empty())).map(str::to_string)
}

fn now() -> String {
    humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string()
}

impl Watch {
    pub(crate) fn new(
        history: History,
        run_id: String,
        tree: Option<String>,
        binary: String,
        resume: bool,
        deputed: Option<prova_core::DeputedRegistry>,
    ) -> Watch {
        Watch {
            history,
            run_id,
            tree,
            binary,
            leaf_source: if resume { "resume" } else { "rerun" },
            deputed,
            events: Vec::new(),
            settled: false,
        }
    }

    /// No history and no deputed account: a watch that can still be asked, and never finds anything.
    pub(crate) fn inert() -> Watch {
        Watch::new(History::default(), crate::journal::new_run_id(), None, String::new(), false, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn event(&self, test: String, deputed: Option<DeputedRef>, failed_run: String, first_failure: Option<String>, source: &str, node: Option<String>) -> FlakeEvent {
        FlakeEvent {
            schema: FLAKE_SCHEMA,
            kind: "flake".into(),
            test,
            deputed,
            tree: self.tree.clone(),
            binary: self.binary.clone(),
            failed_run,
            passed_run: self.run_id.clone(),
            first_failure,
            source: source.into(),
            at: now(),
            node,
        }
    }

    /// A leaf passed here. When an earlier run on this tree failed it, that is a flake: recorded,
    /// and the (failed run, first failure) returned so the journal keeps the origin.
    pub(crate) fn leaf_passed(&mut self, key: &str, node: &str) -> Option<(String, Option<String>)> {
        let (failed_run, first) = self.history.leaves.get(key)?.clone();
        let ev = self.event(key.to_string(), None, failed_run.clone(), first.clone(), self.leaf_source, Some(node.to_string()));
        self.events.push(ev);
        Some((failed_run, first))
    }

    /// Judge the deputed account once the run is over: a case the deputy marked `flaky` is a flake
    /// (`resume` when a merge names the run it failed in, else the deputy's own `junit-retry`), and a
    /// case that PASSED here after an earlier run on this tree failed it is re-marked `flaky`.
    pub(crate) fn settle_deputed(&mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        let Some(registry) = self.deputed.clone() else { return };
        let mut cases = registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut found = Vec::new();
        for c in cases.iter_mut() {
            let key = (c.verifier.clone(), c.suite.clone(), c.name.clone());
            let (failed_run, first, source) = match c.outcome.as_str() {
                "flaky" => match &c.flaky_from {
                    Some(from) => (from.clone(), c.message.clone(), "resume"),
                    None => (self.run_id.clone(), c.message.clone(), "junit-retry"),
                },
                "passed" if c.reused_from.is_none() => match self.history.deputed.get(&key) {
                    Some((from, msg)) => {
                        c.outcome = "flaky".into();
                        c.flaky_from = Some(from.clone());
                        if c.message.is_none() {
                            c.message = msg.clone();
                        }
                        (from.clone(), msg.clone(), "rerun")
                    }
                    None => continue,
                },
                _ => continue,
            };
            let deputed = DeputedRef { verifier: key.0, suite: key.1, name: key.2 };
            let test = format!("{}#{}", deputed.suite, deputed.name);
            found.push((test, deputed, failed_run, first_line(first.as_deref()), source));
        }
        drop(cases);
        for (test, deputed, failed_run, first, source) in found {
            let ev = self.event(test, Some(deputed), failed_run, first, source, None);
            self.events.push(ev);
        }
    }

    /// Put this run's flakes on its summary: a flaky leaf leaves `passed` (it is not evidence), and
    /// every flake — leaf or deputed — is counted and listed.
    pub(crate) fn apply(&self, summary: &mut prova_core::Summary) {
        let leaves = self.events.iter().filter(|e| e.deputed.is_none()).count();
        summary.passed = summary.passed.saturating_sub(leaves);
        summary.flaky = self.events.len();
        summary.flakes = self.events.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, outcome: &str, flaky_from: Option<&str>) -> prova_core::DeputedCase {
        prova_core::DeputedCase {
            verifier: "junit".into(),
            suite: "s".into(),
            name: name.into(),
            outcome: outcome.into(),
            message: None,
            time_ms: None,
            file: "j.xml".into(),
            reused_from: None,
            flaky_from: flaky_from.map(str::to_string),
        }
    }

    /// Two runs on one tree: the leaf that failed before and passes now is a flake that names both
    /// runs; one that never failed is not. Its summary leaves `passed` and enters `flaky`.
    #[test]
    fn a_pass_after_a_failure_on_the_same_tree_is_flaky() {
        let mut history = History::default();
        history.leaves.insert("f › flaky".into(), ("run-1".into(), Some("boom".into())));
        let mut w = Watch::new(history, "run-2".into(), Some("jj:abc".into()), "bin".into(), false, None);
        assert!(w.leaf_passed("f › steady", "steady").is_none());
        assert_eq!(w.leaf_passed("f › flaky", "flaky"), Some(("run-1".into(), Some("boom".into()))));
        let e = &w.events[0];
        assert_eq!((e.failed_run.as_str(), e.passed_run.as_str(), e.source.as_str()), ("run-1", "run-2", "rerun"));
        let mut s = prova_core::Summary { passed: 2, ..Default::default() };
        w.apply(&mut s);
        assert_eq!((s.passed, s.flaky), (1, 1));
    }

    /// The deputed account: a retried case is `junit-retry`, a merged one `resume`, a pass over an
    /// earlier run's red is re-marked flaky (`rerun`), and an untouched pass stays passed.
    #[test]
    fn deputed_flakes_are_judged_once_with_their_sources() {
        let registry: prova_core::DeputedRegistry = std::sync::Arc::default();
        registry.lock().unwrap().extend([
            case("retried", "flaky", None),
            case("merged", "flaky", Some("run-0")),
            case("rerun", "passed", None),
            case("steady", "passed", None),
        ]);
        let mut history = History::default();
        history.deputed.insert(("junit".into(), "s".into(), "rerun".into()), ("run-1".into(), Some("red".into())));
        let mut w = Watch::new(history, "run-2".into(), None, "bin".into(), false, Some(registry.clone()));
        w.settle_deputed();
        w.settle_deputed();
        let sources: Vec<_> = w.events.iter().map(|e| (e.test.as_str(), e.source.as_str(), e.failed_run.as_str())).collect();
        assert_eq!(sources, [("s#retried", "junit-retry", "run-2"), ("s#merged", "resume", "run-0"), ("s#rerun", "rerun", "run-1")]);
        let cases = registry.lock().unwrap();
        assert_eq!(cases[2].outcome, "flaky", "the record shows the re-marked case as flaky");
        assert_eq!(cases[3].outcome, "passed");
    }

    /// A prior record lends its deputed failures only over the same tree AND binary.
    #[test]
    fn a_different_tree_lends_no_history() {
        let rec: prova_core::ledger::Record = serde_json::from_str(
            r#"{"schema":5,"version":"x","binary":"bin","run_id":"run-1","tree":"jj:abc","selection":[],
                "duration_ms":0,"summary":{},"executed":{},"skipped":[],"deselected":[],
                "deputed":[{"verifier":"junit","suite":"s","name":"t","outcome":"failed","file":"j"}]}"#,
        )
        .unwrap();
        assert_eq!(History::deputed_from(&rec, "jj:abc", "bin").len(), 1);
        assert!(History::deputed_from(&rec, "jj:other", "bin").is_empty(), "another tree");
        assert!(History::deputed_from(&rec, "jj:abc", "other").is_empty(), "another prova");
    }
}
