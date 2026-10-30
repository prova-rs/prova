//! The run's conclusion (split from `dispatch` for size): the record, the topology-attach decree,
//! baselines, the empty-selection error, the attention section, snapshot reconciliation, the flake
//! verdict, and the exit code.

use super::*;

/// Everything after the suites ran: the record, the topology-attach decree, baselines, the
/// empty-selection error, the attention section, snapshot reconciliation, and the exit code.
#[allow(clippy::too_many_arguments)]
pub(super) fn conclude_run(
    cli: &Cli,
    home: &Option<Home>,
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    mut summary: prova_core::Summary,
    mut reporter: FailureRecorder,
    accounts: Accounts,
    env_heed: crate::manifest::Heed,
    budget: Option<std::time::Duration>,
    from_manifest: bool,
    is_console: bool,
    journal_plan: Option<&JournalPlan>,
) -> ExitCode {
    store_last_failed(home, &reporter.failed);
    // The engine's summary knows one run; the flake check knows this tree's history.
    reporter.flakes.settle_deputed();
    reporter.flakes.apply(&mut summary);

    // Drain this run's measurements once, up front: they feed the attention account (a
    // reminder condition can read them — the pre-authorship surface of the same claim a
    // ratchet gates), the record (history), and the guarded baseline writer below.
    let mut measurements = drain(&accounts.measurements);
    // The run's wall time joins the account as an ordinary measurement
    // (docs/design/reminders.md#duration-drift-is-attention): a reminder watches its drift, a
    // ratchet can gate it, and banking it is the same deliberate `--update-baseline` act as any
    // metric — in its own `timings` set, so the machine-varying number is a visible, separable
    // file. Recorded, never enforced: slowness is attention, not death.
    measurements.push(prova_core::Measurement {
        name: "run.duration_ms".to_string(),
        value: summary.duration.as_millis() as f64,
        direction: prova_core::Direction::LowerIsBetter,
        set: "timings".to_string(),
    });
    // What contention cost this run (docs/design/agent-ergonomics.md#narrate-lock-waits): the wall
    // time it spent STALLED on a cross-instance lock — the amount that would come back if the
    // holder elsewhere went away. Banked on every run, including as a zero, because a metric that
    // is absent when nothing happened cannot be told from one that was never measured, and a
    // baseline cannot hold it. Narration explains a single run; this is what a reminder watches
    // drift on as concurrent agents make contention the norm.
    measurements.push(prova_core::Measurement {
        name: "run.lock_wait_ms".to_string(),
        value: prova_core::locks::take_stalled().as_millis() as f64,
        direction: prova_core::Direction::LowerIsBetter,
        set: "timings".to_string(),
    });

    // The reports line, in the recap's spirit and for the same reason as `switched off:` — an
    // artifact nobody knows exists is one nobody reads. Peeked BEFORE the record drains the
    // account, console only, and silent when nothing published: a run that produces no artifact
    // should say nothing rather than say "none".
    if is_console {
        let published = accounts
            .reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !published.is_empty() {
            let names = published
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            println!("reports: {names} — read with `prova reports <name>`");
        }
    }

    // A resumed run is not a full one for the attention account: its reused tests took no
    // measurements and emitted no outcomes, so a condition would read a partial ledger.
    let full_run = from_manifest && config.selection.is_empty() && !cli.falsify && !cli.promises_only
        && config.reuse.is_empty();
    let reminders = store_run_record(
        cli, home, suites, config, &summary, &mut reporter, &accounts, full_run, &measurements,
        journal_plan,
    );

    // `--topology NAME` insisted on attaching — a suite that never declared the topology
    // ran against nothing held, which is exactly what the flag exists to prevent.
    if let Some(want) = &cli.require_topology {
        let bound = accounts
            .attached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|n| n == want);
        if !bound {
            eprintln!(
                "prova: --topology {want:?}: the suite never declared it (prova.topology({want:?}, …)) — nothing ran against the held instance"
            );
            return ExitCode::from(2);
        }
    }

    // `--update-baseline`: move the committed baselines toward this run's observed values,
    // through the guard and the steady-state policy (first-sights establish; bare tightens only
    // goal-carrying metrics; =names moves exactly those; loosening is never an option here).
    if let Some(selection) = &cli.update_baseline {
        match home.as_ref() {
            Some(h) => {
                prova_core::baselines::update(&h.dir, &measurements, selection).print();
            }
            None => eprintln!("prova: --update-baseline: no project home; nothing to write"),
        }
    }

    // Open promises COUNT as matched: `--node "<a promised test>"` selected and ran that
    // node — its body being expectedly red is the promise mechanism, not an empty
    // selection. Field-reported: the error fired after a PROMISED node was plainly shown.
    let ran = summary.passed + summary.failed + summary.skipped + summary.promised + summary.flaky;
    if ran == 0 && !config.selection.is_empty() && !cli.allow_empty {
        report_empty_selection(&config.selection, summary.deselected);
        return ExitCode::from(2);
    }

    // The attention section, after the evidence summary — console only, and only when
    // freshly evaluated (a carried-forward account was already reported by the run that
    // evaluated it; re-printing it here would date-stamp stale news as this run's).
    if is_console && full_run {
        print_reminders(&reminders);
    }

    // Reconcile unreferenced snapshots (only when tracking was enabled on a full run).
    let orphaned = reconcile_unreferenced(accounts.snapshots.as_ref(), &cli.unreferenced);

    // DUE is non-fatal by default — the world moving is not a defect in the change under
    // test. A context that promised attention fails on the DUE reminders it heeds: `heed` in
    // the manifest / a profile, plus this invocation's `--heed`, unioned. Selective heed
    // (`heed = ["line-counts"]`, `--heed=clippy`) gates only the matching DUE reminders.
    let effective_heed = env_heed.merge(cli.heed.clone());
    let heeded_due = reminders
        .iter()
        .filter(|e| e.is_due() && effective_heed.heeds(e))
        .count();
    if heeded_due > 0 {
        let plural = if heeded_due == 1 { "" } else { "s" };
        eprintln!(
            "prova: {heeded_due} heeded reminder{plural} due — this context heeds the \
             attention account (heed / --heed); see `prova reminders`"
        );
        return ExitCode::FAILURE;
    }

    // The lane's time budget (docs/design/manifest.md#lane-time-budgets): the manifest states
    // the bar this composition promises, so exceeding it is red even all-green — a
    // conduct-carrying proof in the wrong lane is caught at placement time, not discovered at
    // usage time. Suite wall time only: the subject provision and collection are not the lane's
    // composition.
    if let Some(budget) = budget {
        if summary.duration > budget {
            eprintln!(
                "prova: lane over budget — the suite ran {:.1?} against a {:.1?} budget \
                 (`budget` in the manifest). A heavy proof in the wrong lane? Switches gate the \
                 heavy legs (`prova learn switches`), and a conduct belongs behind one.",
                summary.duration, budget
            );
            return ExitCode::FAILURE;
        }
    }

    if summary.is_success() && !(cli.unreferenced == "warn" && orphaned) {
        // A flake is a defect, never "load": by default it fails an otherwise green run.
        crate::flakes::exit_for(summary.flaky, cli.flakes_warn)
    } else {
        ExitCode::FAILURE
    }
}
