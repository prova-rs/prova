//! Execution: drive the plan's leaves (tests, flows) through the Lua state and
//! collect NodeResults into the run summary.

use super::*;

pub(super) struct NodeResult {
    pub(super) path: String,
    pub(super) outcome: Outcome,
    pub(super) duration: Duration,
    pub(super) assertions: usize,
    pub(super) message: Option<String>,
    /// Source location of the declaration (file path + 1-based line), when the leaf has file
    /// backing — threaded into `Event::NodeFinished` for reporters.
    pub(super) file: Option<String>,
    pub(super) line: Option<u32>,
    /// True for a `⟶ teardown` leaf.
    ///
    /// Reported exactly like any other node — but it **never gates**. A cleanup that raised is not
    /// the work failing: the body already passed. So it must not cascade-skip a flow's later steps,
    /// and must not skip a `depends_on` dependent, either of which would report a defect in code
    /// that is fine. It is reported *because it happened*, not because the work failed. The flag
    /// makes that structural rather than positional — the first proof written here caught the
    /// alternative (keying on "any failed result") skipping a flow's remaining steps.
    pub(super) teardown: bool,
    /// The spec flag's reason for an `Outcome::Promised` result (set by the inversion, threaded into
    /// `Event::NodeFinished::promise_reason`). `None` for every other outcome.
    pub(super) promises: Option<String>,
}

/// The spec outcome inversion, applied to a promises-flagged leaf's results after it ran
/// (docs/plans/api-freeze.md §5). Teardown results are exempt — they report cleanup, not the work.
///
/// - Any work result **failed** → the leaf is an **open promise**: each failure becomes
///   `Outcome::Promised` (CI green) — unless `strict` (driver mode), where open promises stay failures.
/// - No failures and ≥1 pass → the spec is **honored**: each pass becomes a *failure* demanding
///   graduation — convert the flag to `proves = "<context>"` (preferred: the reason lives on in
///   the test) or remove it — so an implementation cannot land still flagged `spec`.
/// - All skipped → untouched: an unmet `requires` wins over spec (nothing was observed).
pub(super) fn apply_spec_inversion(results: &mut [NodeResult], reason: &str, strict: bool) {
    let failed = results
        .iter()
        .any(|r| !r.teardown && r.outcome == Outcome::Failed);
    if failed {
        if !strict {
            for r in results
                .iter_mut()
                .filter(|r| !r.teardown && r.outcome == Outcome::Failed)
            {
                r.outcome = Outcome::Promised;
                r.promises = Some(reason.to_string());
            }
        }
        return;
    }
    // The graduation fix is copy-pasteable: the promise's (always non-empty) reason becomes the
    // proves context. Graduation is a tense change — promises → proves — and the message carries
    // the exact replacement so keeping a promise and recording it land in one edit.
    let fix = format!("proves = {reason:?}");
    for r in results
        .iter_mut()
        .filter(|r| !r.teardown && r.outcome == Outcome::Passed)
    {
        r.outcome = Outcome::Failed;
        r.message = Some(format!(
            "promise kept — change `promises` to {fix} (keep the context) or remove the flag"
        ));
    }
}

/// Snapshot context for one test: where its `.snap` files live and how they're keyed. `None`
/// when the source file has no recorded path (e.g. a topology run), which makes
/// `matches_snapshot` error rather than guess.
fn snapshot_ctx_for(state: &RunState, item: &PlanItem) -> Option<SnapshotCtx> {
    let dir = state.snapshot_dir(item.file)?;
    let stem = state
        .file_paths
        .get(item.file)
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("tests")
        .to_string();
    Some(SnapshotCtx {
        dir,
        stem,
        key_base: slugify(&item.path),
        update: state.update_snapshots,
        counter: 0,
        registry: state.snapshot_registry.clone(),
    })
}

/// Invert an outcome under falsification: going red is the proof succeeding. A body that
/// survives its own mutation asserts nothing about the system — it is vacuous, and reporting it
/// green would let it keep counting as evidence forever. A falsifier that itself raised is a
/// failure of the mutation, not of the proof, and says so.
fn invert_for_falsify(
    outcome: Outcome,
    message: Option<String>,
    falsifier_error: Option<String>,
) -> (Outcome, Option<String>) {
    match falsifier_error {
        Some(err) => (Outcome::Failed, Some(err)),
        None => match outcome {
            Outcome::Failed => (Outcome::Passed, None),
            Outcome::Passed => (
                Outcome::Failed,
                Some(
                    "vacuous — the body still passed with its falsifier applied, so it is not \
                     asserting what the mutation breaks. Sharpen the assertion, or fix the \
                     falsifier to break what the proof actually checks."
                        .to_string(),
                ),
            ),
            // A skip observed nothing, so there is nothing to invert.
            other => (other, message),
        },
    }
}

/// The falsification pass: break the system FIRST, then run the body against the wreckage. A
/// falsifier that itself raises is a failure of the mutation, not of the proof, and says so —
/// otherwise a broken falsifier would masquerade as a body that correctly went red.
async fn run_falsifier(
    item: &PlanItem,
    state: &Rc<RunState>,
    ctx_ud: &mlua::AnyUserData,
) -> Option<String> {
    if !state.falsify {
        return None;
    }
    match &item.falsifier {
        Some(f) => match f.call_async::<()>(ctx_ud.clone()).await {
            Ok(()) => None,
            Err(e) => Some(format!("falsifier raised before the body ran: {e}")),
        },
        None => None,
    }
}

/// Returns the test's own node, plus a `⟶ teardown` node per teardown failure (usually none).
pub(super) async fn run_one(
    lua: &Lua,
    item: &PlanItem,
    state: &Rc<RunState>,
    flow_scope: Option<Rc<RefCell<ScopeState>>>,
) -> Vec<NodeResult> {
    let run = Rc::new(RefCell::new(TestRun::default()));
    run.borrow_mut().snapshot = snapshot_ctx_for(state, item);
    let test_scope = Rc::new(RefCell::new(ScopeState::default()));
    // The case is delivered both as `t.case` and as the body's second argument, so `fn(t, case)`
    // and `fn(t)` (ignoring the trailing nil) both work.
    let case_arg = item.case.clone().unwrap_or(Value::Nil);
    let ctx = Ctx {
        run: run.clone(),
        state: state.clone(),
        test_scope: test_scope.clone(),
        file_scope: state.file_scope(item.file),
        flow_scope,
        own_scope: ScopeKind::Test,
        case: item.case.clone(),
        topology: false,
    };
    let ctx_ud = match lua.create_userdata(ctx) {
        Ok(ud) => ud,
        // No context, no test: report the failure as this node's result instead of panicking
        // the whole worker — the one honest outcome a broken allocation has.
        Err(e) => {
            return vec![NodeResult {
                path: item.path.clone(),
                outcome: Outcome::Failed,
                duration: std::time::Duration::ZERO,
                assertions: 0,
                message: Some(format!("cannot create the test context: {e}")),
                file: state.file_path_str(item.file),
                line: item.line,
                teardown: false,
                promises: None,
            }];
        }
    };

    let file = state.file_path_str(item.file);
    let start = Instant::now();

    let falsifier_error = run_falsifier(item, state, &ctx_ud).await;

    let call = item.body.call_async::<()>((ctx_ud, case_arg));

    let result = match item.timeout {
        Some(budget) => match tokio::time::timeout(budget, call).await {
            Ok(r) => r,
            Err(_elapsed) => {
                let assertions = run.borrow().assertions;
                // Teardown still runs after a timeout — and a timed-out test is exactly when a
                // cleanup is most likely to raise, so its errors are reported rather than dropped.
                let errors = teardown_scope(&test_scope).await;
                let mut out = vec![NodeResult {
                    path: item.path.clone(),
                    outcome: Outcome::Failed,
                    duration: start.elapsed(),
                    assertions,
                    message: Some(format!("timed out after {budget:?}")),
                    file: file.clone(),
                    line: item.line,
                    teardown: false,
                    promises: None,
                }];
                out.extend(teardown_results(&item.path, errors, file.as_deref(), item.line));
                return out;
            }
        },
        None => call.await,
    };
    let duration = start.elapsed();

    let (outcome, message, assertions) = {
        let r = run.borrow();
        let (outcome, message) = if r.skip.is_some() {
            (Outcome::Skipped, r.skip.clone())
        } else if let Err(err) = &result {
            (
                Outcome::Failed,
                Some(r.failure.clone().unwrap_or_else(|| err.to_string())),
            )
        } else {
            (Outcome::Passed, None)
        };
        (outcome, message, r.assertions)
    };

    let (outcome, message) = if state.falsify && item.falsifier.is_some() {
        invert_for_falsify(outcome, message, falsifier_error)
    } else {
        (outcome, message)
    };

    let errors = teardown_scope(&test_scope).await;

    let mut out = vec![NodeResult {
        path: item.path.clone(),
        outcome,
        duration,
        assertions,
        message,
        file: file.clone(),
        line: item.line,
        teardown: false,
        promises: None,
    }];
    out.extend(teardown_results(&item.path, errors, file.as_deref(), item.line));
    out
}

/// A flow is one unit: its steps run serially, in order, on one worker, sharing a `flow`-scope
/// instance. Once a step fails, the remaining steps **cascade-skip** (skip, not fail) with the
/// failing step named. A self-`skip` does not cascade — skip is not failure. The flow scope tears
/// down after the last step (each step's `test` scope having already torn down per-step).
pub(super) async fn run_flow(lua: &Lua, steps: &[PlanItem], state: &Rc<RunState>) -> Vec<NodeResult> {
    let flow_scope = Rc::new(RefCell::new(ScopeState::default()));
    let mut results = Vec::with_capacity(steps.len());
    let mut cascade: Option<String> = None;

    for step in steps {
        if let Some(reason) = &cascade {
            results.push(NodeResult {
                path: step.path.clone(),
                outcome: Outcome::Skipped,
                duration: Duration::ZERO,
                assertions: 0,
                message: Some(reason.clone()),
                file: state.file_path_str(step.file),
                line: step.line,
                teardown: false,
                promises: None,
            });
            continue;
        }
        let step_results = run_one(lua, step, state, Some(flow_scope.clone())).await;
        if step_results
            .iter()
            .any(|r| !r.teardown && r.outcome == Outcome::Failed)
        {
            let failed = step_name(&step.path);
            cascade = Some(format!("skipped: earlier step {failed:?} failed"));
        }
        results.extend(step_results);
    }

    let errors = teardown_scope(&flow_scope).await;
    let label = steps
        .first()
        .map(|s| flow_label(&s.path))
        .unwrap_or("flow")
        .to_string();
    let file = steps.first().and_then(|s| state.file_path_str(s.file));
    results.extend(teardown_results(&label, errors, file.as_deref(), None));
    results
}

/// A flow's own name — the step path minus its trailing step segment.
pub(super) fn flow_label(step_path: &str) -> &str {
    match step_path.rfind(" › ") {
        Some(i) => &step_path[..i],
        None => step_path,
    }
}

/// The last path segment — the step's own name, for the cascade-skip message.
pub(super) fn step_name(path: &str) -> &str {
    path.rsplit(" › ").next().unwrap_or(path)
}

pub(super) async fn run_unit(lua: &Lua, unit: &PlanUnit, state: &Rc<RunState>) -> Vec<NodeResult> {
    match unit {
        PlanUnit::Test(item) => run_one(lua, item, state, None).await,
        PlanUnit::Flow { steps } => run_flow(lua, steps, state).await,
    }
}

/// The unit-level outcome used for dependency gating: a unit failed if any of its leaf results
/// failed; else passed if any passed; else it was entirely skipped.
pub(super) fn unit_outcome(results: &[NodeResult]) -> Outcome {
    // Teardown leaves are excluded: `depends_on` gates on whether the unit's *work* passed, and a
    // dependent's premise ("the upstream did its job") still holds when only a cleanup raised.
    // Gating on it would cascade-skip a whole subgraph over a leaked container.
    //
    // An open promise (`Outcome::Promised`) gates like a failure: the upstream did NOT do its job — its
    // implementation doesn't exist yet — so a dependent's premise cannot hold. Only the *report*
    // treats an open promise gently; the DAG does not.
    let work = || results.iter().filter(|r| !r.teardown);
    if work().any(|r| matches!(r.outcome, Outcome::Failed | Outcome::Promised)) {
        Outcome::Failed
    } else if work().any(|r| r.outcome == Outcome::Passed) {
        Outcome::Passed
    } else {
        Outcome::Skipped
    }
}

/// Build skipped results for a unit that never ran (a dependency did not pass) — one per reported
/// path (a flow reports one skip per step), so the report stays consistent with a unit that ran.
pub(super) fn skip_leaf(unit: &PlanUnit, reason: &str, state: &RunState) -> Vec<NodeResult> {
    unit.items()
        .into_iter()
        .map(|item| NodeResult {
            path: item.path.clone(),
            outcome: Outcome::Skipped,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some(reason.to_string()),
            file: state.file_path_str(item.file),
            line: item.line,
            teardown: false,
            promises: None,
        })
        .collect()
}

pub(super) fn emit_finished(reporter: &mut dyn Reporter, summary: &mut Summary, results: &[NodeResult]) {
    for result in results {
        summary.tally(result.outcome);
        reporter.event(&Event::NodeFinished {
            path: &result.path,
            outcome: result.outcome,
            duration: result.duration,
            assertions: result.assertions,
            message: result.message.as_deref(),
            file: result.file.as_deref(),
            line: result.line,
            promise_reason: result.promises.as_deref(),
        });
    }
}

/// A readers-writer accounting table over resource tokens. Per token it tracks how many shared
/// (reader) and exclusive (writer) holds are live. A reader may acquire when there is no writer; a
/// writer may acquire only when there is neither reader nor writer. Acquisition is all-or-nothing
/// per leaf (checked before any hold is taken), so no leaf ever holds-and-waits — hence no deadlock.
#[derive(Default)]
pub(super) struct ResourceTable {
    pub(super) holders: HashMap<String, (u32, u32)>, // token -> (shared, exclusive)
}

impl ResourceTable {
    fn can_acquire(&self, reqs: &[ResourceReq]) -> bool {
        reqs.iter().all(|r| {
            let (shared, exclusive) = self.holders.get(&r.token).copied().unwrap_or((0, 0));
            if r.shared {
                exclusive == 0
            } else {
                shared == 0 && exclusive == 0
            }
        })
    }

    fn acquire(&mut self, reqs: &[ResourceReq]) {
        for r in reqs {
            let entry = self.holders.entry(r.token.clone()).or_insert((0, 0));
            if r.shared {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
    }

    fn release(&mut self, reqs: &[ResourceReq]) {
        for r in reqs {
            if let Some(entry) = self.holders.get_mut(&r.token) {
                if r.shared {
                    entry.0 = entry.0.saturating_sub(1);
                } else {
                    entry.1 = entry.1.saturating_sub(1);
                }
            }
        }
    }
}

/// Dependency- and resource-aware scheduler. A leaf runs once all its dependency leaves have
/// **passed** *and* its declared resources can be acquired (readers-writer); if any dependency
/// failed or was skipped, the leaf cascade-skips (transitively). Independent, resource-compatible
/// leaves run concurrently up to `config.concurrency`; with the default of 1 this is
/// definition-order sequential and resource declarations are inert.
pub(super) async fn run_plan(
    lua: &Lua,
    plan: &Plan,
    state: &Rc<RunState>,
    config: &RunConfig,
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
) {
    let leaves = &plan.leaves;
    let n = leaves.len();
    let concurrency = config.concurrency.max(1);
    let mut outcome: Vec<Option<Outcome>> = vec![None; n];
    let mut started = vec![false; n];
    let mut resources = ResourceTable::default();
    let mut in_flight = futures::stream::FuturesUnordered::new();

    loop {
        // Skip to a fixpoint: a leaf is skipped without running when it has an unmet `requires`
        // (a precondition skip, independent of deps) or all its deps are resolved but not all passed
        // (a cascade skip). Looping catches transitive skips in one pass.
        let mut changed = true;
        while changed {
            changed = false;
            for i in 0..n {
                if started[i] || outcome[i].is_some() {
                    continue;
                }
                let reason = if let Some(reason) = &leaves[i].precondition_skip {
                    Some(reason.clone())
                } else if !leaves[i].deps.iter().all(|&d| outcome[d].is_some()) {
                    None // deps not all resolved yet — decide later
                } else {
                    leaves[i]
                        .deps
                        .iter()
                        .find(|&&d| outcome[d] != Some(Outcome::Passed))
                        .map(|&blocker| {
                            format!(
                                "skipped: dependency {:?} did not pass",
                                unit_name(&leaves[blocker])
                            )
                        })
                };
                if let Some(reason) = reason {
                    let results = skip_leaf(&leaves[i].unit, &reason, state);
                    for path in leaves[i].unit.leaf_paths() {
                        reporter.event(&Event::NodeStarted { path });
                    }
                    emit_finished(reporter, summary, &results);
                    outcome[i] = Some(Outcome::Skipped);
                    started[i] = true;
                    changed = true;
                }
            }
        }

        // Launch runnable leaves — all deps passed and resources acquirable — up to the concurrency
        // limit. A resource-blocked leaf is left for a later round (a completion frees its holds).
        for i in 0..n {
            if in_flight.len() >= concurrency {
                break;
            }
            if started[i] || outcome[i].is_some() {
                continue;
            }
            if !leaves[i]
                .deps
                .iter()
                .all(|&d| outcome[d] == Some(Outcome::Passed))
            {
                continue;
            }
            if !resources.can_acquire(&leaves[i].reqs) {
                continue;
            }
            resources.acquire(&leaves[i].reqs);
            started[i] = true;
            for path in leaves[i].unit.leaf_paths() {
                reporter.event(&Event::NodeStarted { path });
            }
            in_flight.push(async move { (i, run_unit(lua, &leaves[i].unit, state).await) });
        }

        if in_flight.is_empty() {
            break; // nothing running and nothing became ready — all leaves resolved
        }

        let Some((i, mut results)) = in_flight.next().await else {
            break; // checked non-empty above; an exhausted stream means all leaves resolved
        };
        resources.release(&leaves[i].reqs);
        // A promises-flagged leaf's results are inverted BEFORE gating and reporting: red → open promise
        // (or a real failure under --due), green → "graduate it". Gating sees the
        // post-inversion truth, so a dependent of an open promise still cascade-skips.
        if let Some(reason) = &leaves[i].promises {
            apply_spec_inversion(&mut results, reason, config.due);
        }
        outcome[i] = Some(unit_outcome(&results));
        emit_finished(reporter, summary, &results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, outcome: Outcome, teardown: bool) -> NodeResult {
        NodeResult {
            path: path.to_string(),
            outcome,
            duration: std::time::Duration::ZERO,
            assertions: 0,
            message: None,
            file: None,
            line: None,
            teardown,
            promises: None,
        }
    }

    /// Path arithmetic for the cascade-skip messages: a flow's label is its path minus the step,
    /// and a step's name is the last segment.
    #[test]
    fn flow_paths_split_on_the_separator() {
        assert_eq!(flow_label("suite › flow › step one"), "suite › flow");
        assert_eq!(flow_label("bare"), "bare");
        assert_eq!(step_name("suite › flow › step one"), "step one");
        assert_eq!(step_name("bare"), "bare");
    }

    /// The dependency gate's verdict: teardown leaves never gate (a leaked container must not
    /// cascade-skip a subgraph), and an open promise gates like a failure — the upstream's
    /// implementation does not exist, so a dependent's premise cannot hold.
    #[test]
    fn unit_outcome_gates_on_work_not_cleanup() {
        assert_eq!(unit_outcome(&[node("a", Outcome::Passed, false)]), Outcome::Passed);
        assert_eq!(
            unit_outcome(&[node("a", Outcome::Passed, false), node("a ⟶ teardown", Outcome::Failed, true)]),
            Outcome::Passed,
            "a raising cleanup does not fail the unit's work"
        );
        assert_eq!(
            unit_outcome(&[node("a", Outcome::Promised, false)]),
            Outcome::Failed,
            "an open promise gates like a failure"
        );
        assert_eq!(unit_outcome(&[node("a", Outcome::Skipped, false)]), Outcome::Skipped);
    }
}
