//! Reminders, `prova attest`, and the evidence account.

use super::*;

pub(crate) fn reminders_subcommand(args: Vec<String>) -> ExitCode {
    for arg in &args {
        if arg == "-h" || arg == "--help" {
            println!(
                "usage: prova reminders [--due | --watching] [-k PATTERN] [--tags TAGS] [--node NAME]\n\
                 \x20      prova reminders burndown       drive DUE reminders to green (a run with --heed)\n\n\
                 Lists every declared `prova.remind`, with the state the last run recorded overlaid:\n\
                 DUE (attention owed — with the condition's why and the instruction), WATCHING\n\
                 (armed, the world holds still), UNEVALUATED (could not run, with the reason), or\n\
                 — (declared, no run has evaluated it yet).\n\n\
                 `--due` / `--watching` narrow to one state; the narrowed report answers only for\n\
                 what it lists, so `--watching` exits 0 even while something else is due. Selectors\n\
                 narrow like every lane: `-k` over name and declaring file, `--node` the exact name,\n\
                 `--tags` the reminder's tags, `!` excludes — composing with the state filters.\n\n\
                 Collects the suite but executes nothing — so it works BEFORE any run (listing what\n\
                 is declared) and shows live states AFTER one. Exits non-zero when a DUE is listed."
            );
            return ExitCode::SUCCESS;
        }
    }
    // Driver: `prova reminders burndown` drives DUE reminders to green — a run with `--heed`, so due
    // reminders fail loud (a run is the only thing that freshly evaluates conditions). Consistent
    // with `prova tests burndown`: a red→green worklist that delegates to the run engine.
    if let Some((first, rest)) = args.split_first() {
        if first == "burndown" {
            let mut full = vec!["--heed".to_string()];
            full.extend(rest.iter().cloned());
            return run(full);
        }
    }
    // States are adjectives on the lane (docs/design/reminders.md#reminders-state-filters). The
    // spellings are rewritten to internal flags before forwarding because the run parser already
    // owns `--due` for something else entirely (promises falling due by decree).
    let mut state: Option<&str> = None;
    let mut rest: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--due" | "--watching" => {
                let want = if arg == "--due" { "due" } else { "watching" };
                if state.is_some_and(|s| s != want) {
                    eprintln!("prova: reminders: --due and --watching are mutually exclusive");
                    return ExitCode::from(2);
                }
                state = Some(want);
            }
            _ => rest.push(arg),
        }
    }
    // Report: route through the run machinery to collect declared reminders (loading the suite, like
    // `--list`, without executing), then overlay the recorded state. One command, before and after.
    let mut full = vec!["--reminders-list".to_string()];
    if let Some(want) = state {
        full.push(format!("--reminders-{want}"));
    }
    full.extend(rest);
    run(full)
}

/// Render the attention account as a listing: every declared reminder, with the last run's recorded
/// state overlaid, or `—` when no run has evaluated it yet. The same rows before and after a run —
/// a run only fills in the state. Exits non-zero only on a recorded DUE.
pub(crate) fn list_reminders(
    declared: &[prova_core::ReminderListing],
    recorded: &[record::ReminderEntry],
    state: Option<&str>,
) -> ExitCode {
    if declared.is_empty() {
        println!(
            "prova: no reminders declared — add `prova.remind(name, {{ when = … }}, message)` to a \
             proof (`prova learn reminders`)"
        );
        return ExitCode::SUCCESS;
    }
    let states: std::collections::BTreeMap<&str, &record::ReminderEntry> =
        recorded.iter().map(|e| (e.name.as_str(), e)).collect();

    // Worst-first: DUE, UNEVALUATED, WATCHING, then the never-run rows last.
    let rank = |d: &prova_core::ReminderListing| match states.get(d.name.as_str()).map(|e| e.state.as_str()) {
        Some("due") => 0,
        Some("unevaluated") => 1,
        Some(_) => 2,
        None => 3,
    };
    let mut order: Vec<&prova_core::ReminderListing> = declared.iter().collect();
    // `--due` / `--watching` narrow to one lane state; the narrowed report speaks only for what it
    // lists (docs/design/reminders.md#reminders-state-filters), so pending/unevaluated rows appear
    // only in the full report and the exit code follows the shown rows.
    if let Some(want) = state {
        order.retain(|d| states.get(d.name.as_str()).is_some_and(|e| e.state == want));
        if order.is_empty() {
            match want {
                "due" => println!("prova: nothing due — the world holds still"),
                _ => println!("prova: nothing watching"),
            }
            return ExitCode::SUCCESS;
        }
    }
    order.sort_by_key(|d| rank(d));

    let (mut due, mut watching, mut unevaluated, mut pending) = (0, 0, 0, 0);
    for d in order {
        match states.get(d.name.as_str()) {
            Some(e) if e.state == "due" => {
                due += 1;
                match &e.why {
                    Some(w) => println!("  {:<12} {} — {w}", "DUE", d.name),
                    None => println!("  {:<12} {}", "DUE", d.name),
                }
                println!("               ↳ {}", d.message);
            }
            Some(e) if e.state == "unevaluated" => {
                unevaluated += 1;
                println!(
                    "  {:<12} {} — {}",
                    "UNEVALUATED",
                    d.name,
                    e.why.as_deref().unwrap_or("could not evaluate")
                );
            }
            Some(_) => {
                watching += 1;
                println!("  {:<12} {}", "WATCHING", d.name);
            }
            None => {
                pending += 1;
                println!("  {:<12} {}", "—", d.name);
            }
        }
    }
    println!();
    match state {
        Some(want) => println!("  {} {want} of {} declared", due + watching, declared.len()),
        None => {
            println!(
                "  {} declared · {due} due, {unevaluated} unevaluated, {watching} watching",
                declared.len()
            );
            if pending > 0 {
                println!("  {pending} not yet evaluated — run `prova` for live status");
            }
        }
    }
    if due > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `prova attest <address>` — did the proof covering this obligation actually execute?
///
/// The question `prova owed` cannot answer. `owed` is static: it reconciles anchors against
/// `covers` bindings and reports that an obligation *has* a proof. Whether that proof ever RAN is a
/// fact about a run, and it is the fact an agent gets wrong — a suite that exits 0 having skipped
/// the only proof for a claim reports covered, honestly, and is wrong.
///
/// Exit 1 when the obligation is not attested; exit 2 for a usage error. The distinction matters in
/// CI: a missing argument is a broken pipeline, an unattested claim is a real finding.
pub(crate) fn attest_subcommand(args: Vec<String>) -> ExitCode {
    let mut address: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova attest [<doc.md#claim-id> | <claim-id>]\n\n\
                     Reports whether the proof covering an obligation actually executed in the last\n\
                     recorded run. A skipped, deselected or absent proof attests nothing.\n\n\
                     A bare claim id resolves when exactly one claim carries it; an ambiguous id\n\
                     lists the candidates. With no address at all: reconcile EVERY anchored claim\n\
                     and exit non-zero unless each one is attested — the CI gate."
                );
                return ExitCode::SUCCESS;
            }
            other if !other.starts_with('-') && address.is_none() => {
                address = Some(other.to_string());
            }
            other => {
                eprintln!("prova: attest: unexpected argument {other:?}\nusage: prova attest [<doc.md#claim-id>]");
                return ExitCode::from(2);
            }
        }
    }

    let home = match resolve_home(None) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let (manifest, packages_resolved) = match resolve_for_obligations(&home) {
        Ok(pair) => pair,
        Err(code) => return code,
    };

    // No address: the pipeline's question. Reconcile the whole account into one exit code.
    let Some(address) = address else {
        return attest_all(&home, &manifest, &packages_resolved);
    };

    if let Some(rest) = address.strip_prefix("junit:") {
        return attest_deputed(&home, &address, rest);
    }
    let address = match resolve_claim_address(&home, &manifest, address) {
        Ok(a) => a,
        Err(code) => return code,
    };

    // No record at all is the absence of evidence, not the absence of a problem. Treating it as
    // fine would make the atom opt-out by simply never running anything.
    let Some(recorded) = record::load(&home) else {
        println!("prova: attest {address}");
        println!("  ↳ no run has been recorded here — run the suite first (`prova`)");
        return ExitCode::FAILURE;
    };

    let proofs = match collect_obligations(&home, &manifest, &packages_resolved) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };

    // A binding may carry a pin (`path#id@digest`); the pin records which prose was accepted, not
    // which proof ran, so it plays no part in matching here.
    let bindings: Vec<String> = proofs
        .iter()
        .filter(|p| {
            p.covers
                .iter()
                .any(|c| claims::split_pin(c).0 == address)
        })
        .map(|p| p.path.clone())
        .collect();

    let verdict = record::attest(&recorded, &bindings);
    println!("prova: attest {address}");
    match &verdict {
        record::Attested::Yes { path } => {
            println!("  ↳ attested — {path} ran and passed");
        }
        record::Attested::Red { path, outcome } => {
            let what = match outcome {
                record::Executed::Failed => "failed",
                record::Executed::Promised => "is an open promise, red by definition",
                record::Executed::Passed => unreachable!("a passing proof attests"),
            };
            println!("  ↳ NOT attested — {path} {what}");
        }
        record::Attested::NoEvidence { path, why } => {
            println!("  ↳ NOT attested — {path} did not execute in the recorded run");
            println!("    ({why})");
        }
        record::Attested::Unbound => {
            println!("  ↳ NOT attested — no proof declares `covers = \"{address}\"`");
            println!("    (`prova owed` lists every obligation and its binding)");
        }
    }
    if verdict.is_attested() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// A deputed address (`junit:<suite>#<name>`) asks about a case another verifier produced
/// (docs/design/verifiers.md): did it execute and pass in the recorded run? Answered from the
/// record's deputed rows — same contract as every other address, red/skipped/absent attest
/// nothing.
fn attest_deputed(home: &home::Home, address: &str, rest: &str) -> ExitCode {
    let (suite, name) = match rest.split_once('#') {
        Some(pair) => pair,
        None => {
            eprintln!("prova: a deputed address is junit:<suite>#<case>, got {address:?}");
            return ExitCode::from(2);
        }
    };
    let Some(recorded) = record::load(home) else {
        println!("prova: attest {address}");
        println!("  ↳ no run has been recorded here — run the suite first (`prova`)");
        return ExitCode::FAILURE;
    };
    println!("prova: attest {address}");
    let row = recorded
        .deputed
        .iter()
        .find(|d| d.verifier == "junit" && d.suite == suite && d.name == name);
    match row {
        Some(d) if d.outcome == "passed" => {
            println!("  ↳ attested — the deputed case ran and passed (from {})", d.file);
            ExitCode::SUCCESS
        }
        Some(d) => {
            match &d.message {
                Some(m) => println!("  ↳ NOT attested — the deputed case {}: {m}", d.outcome),
                None => println!("  ↳ NOT attested — the deputed case {}", d.outcome),
            }
            ExitCode::FAILURE
        }
        None => {
            println!(
                "  ↳ NOT attested — no ingested case matches ({} deputed rows in the record)",
                recorded.deputed.len()
            );
            ExitCode::FAILURE
        }
    }
}

/// A bare id resolves when exactly one claim carries it. The full address stays canonical —
/// an agent has it in its buffer — but ids are the memorable half, and a human should not
/// have to copy/paste a path to ask about one claim. Zero matches falls through untouched:
/// a ticket address (`covers = "PROJ-123"`) has no `#` and no anchor, and must keep working.
fn resolve_claim_address(
    home: &home::Home,
    manifest: &Manifest,
    address: String,
) -> Result<String, ExitCode> {
    if address.contains('#') {
        return Ok(address);
    }
    let docs = spec_scan_roots(manifest);
    let scanned = match claims::scan(&home.dir, &docs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("prova: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    let matches = claims::matching_id(&scanned, &address);
    match matches.len() {
        1 => Ok(matches[0].address.clone()),
        0 => Ok(address),
        n => {
            println!("prova: attest {address}");
            println!("  ambiguous — {n} claims match:");
            for m in matches {
                println!("    {}", m.address);
            }
            Err(ExitCode::from(2))
        }
    }
}

/// Resolve a package the way a RUN resolves it, for the verbs that only read obligations
/// (`owed`, `attest`). One door, so the ledger can never disagree with the suite about what a
/// package is — the divergence that made both verbs crash on any package with local plugins.
///
/// `require_proofs = false`: these verbs read whatever proofs exist and have nothing to select, so
/// a plugins-only manifest is legitimate here where it would be a config error for a run.
pub(crate) fn resolve_for_obligations(
    home: &home::Home,
) -> Result<(Manifest, packages::ResolvedPackages), ExitCode> {
    let manifest = read_manifest(home)?;
    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("prova: {e}");
            return Err(ExitCode::from(2));
        }
    };
    let run = resolve_from_manifest(home, None, None, None, None, &layout, false, false, false)?;
    Ok((manifest, run.dependencies))
}

/// The attention-account pass at the end of a run (docs/design/reminders.md): build the account
/// this run earned, evaluate every `prova.remind` condition against it, and return the record rows.
///
/// The account's `owed` is the ledger's remainder exactly as `prova owed` counts it — open
/// promises, unproven claims, dangling covers — and deliberately NOT reminders: a condition must
/// never observe reminder state (one pass, no fixpoint). If the ledger cannot be reconciled the
/// pass is skipped with a note and the previous rows carry forward — a wrong `owed` would fire
/// ledger conditions falsely, which is worse than firing late.
pub(crate) fn evaluate_run_reminders(
    home: &home::Home,
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    summary: &prova_core::Summary,
    measurements: &[prova_core::Measurement],
) -> Vec<record::ReminderEntry> {
    // One scan feeds both the owed count and the dated obligations a draw-down condition reads —
    // the anchors are already in hand, so exposing their deadlines costs nothing extra.
    let reconciled = (|| -> Result<(usize, Vec<prova_core::DatedObligation>), String> {
        let (manifest, packages_resolved) =
            resolve_for_obligations(home).map_err(|_| "could not resolve the package".to_string())?;
        let docs = spec_scan_roots(&manifest);
        let claims = claims::scan(&home.dir, &docs).map_err(|e| e.to_string())?;
        let proofs = collect_obligations(home, &manifest, &packages_resolved)?;
        let owed = claims::reconcile(&claims, &proofs).len();
        let dated = claims
            .iter()
            .filter_map(|c| {
                c.date.as_ref().map(|d| prova_core::DatedObligation {
                    address: c.address.clone(),
                    date: d.clone(),
                    kind: match c.kind {
                        claims::Kind::Backlog => "backlog",
                        claims::Kind::Claim => "claim",
                    }
                    .to_string(),
                })
            })
            .collect();
        Ok((owed, dated))
    })();
    let (owed, dated) = match reconciled {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("prova: reminders not evaluated — could not reconcile the ledger: {e}");
            return record::load(home).map(|r| r.reminders).unwrap_or_default();
        }
    };
    let account = prova_core::ReminderAccount {
        passed: summary.passed,
        failed: summary.failed,
        skipped: summary.skipped,
        promised: summary.promised,
        owed,
        dated,
        measurements: measurements
            .iter()
            .map(|m| (m.name.clone(), m.value))
            .collect(),
    };
    record::reminder_entries(&prova_core::evaluate_reminders(suites, config, &account))
}

/// Print the attention section after the run summary — console format only (JSON/TAP streams are
/// the evidence account and never carry reminders). WATCHING is silence, by design; DUE prints
/// loud with the condition's why and the instruction; UNEVALUATED prints its reason, because a
/// watcher that could not look must stay visibly disarmed.
pub(crate) fn print_reminders(entries: &[record::ReminderEntry]) {
    let due: Vec<_> = entries.iter().filter(|e| e.state == "due").collect();
    let unevaluated: Vec<_> = entries.iter().filter(|e| e.state == "unevaluated").collect();
    if due.is_empty() && unevaluated.is_empty() {
        return;
    }
    println!();
    for e in &due {
        match &e.why {
            Some(w) => println!("  DUE  {} — {w}", e.name),
            None => println!("  DUE  {}", e.name),
        }
        println!("       ↳ {}", e.message);
    }
    for e in &unevaluated {
        println!(
            "  UNEVALUATED  {} — {}",
            e.name,
            e.why.as_deref().unwrap_or("could not evaluate")
        );
    }
    println!();
    let plural = if due.len() == 1 { "" } else { "s" };
    let mut line = format!("  {} reminder{plural} due", due.len());
    if !unevaluated.is_empty() {
        line.push_str(&format!(", {} unevaluated", unevaluated.len()));
    }
    println!("{line}");
}

/// How the run was narrowed, spelled the way it was asked for.
///
/// An empty list is the load-bearing case: only a record with no selection at all can speak for the
/// whole suite. Anything else is a statement about a subset, and the record has to carry enough for
/// a reader to see which one.
pub(crate) fn spell_selection(config: &prova_core::RunConfig) -> Vec<String> {
    let sel = &config.selection;
    let mut out = Vec::new();
    out.extend(sel.keywords.iter().map(|k| format!("-k {k}")));
    out.extend(sel.keyword_excludes.iter().map(|k| format!("-k !{k}")));
    out.extend(sel.tags.iter().map(|t| format!("--tags {t}")));
    out.extend(sel.tag_excludes.iter().map(|t| format!("--tags !{t}")));
    out.extend(sel.nodes.iter().map(|n| format!("--node {n}")));
    if config.promises_only {
        out.push("--promises".to_string());
    }
    if config.falsify {
        out.push("--falsify".to_string());
    }
    out
}

/// `prova attest` with no address — every anchored claim, one exit code.
///
/// The developer's question is one address; the pipeline's question is "is everything this
/// project claims actually evidenced", and it has to be a single exit code or CI cannot gate on
/// it. Any claim that is unbound, or whose covering proof did not execute and pass in the
/// recorded run, fails the gate. A package with no claims exits 0 with a stated reason — a
/// pipeline wiring the gate before declaring `[specs]` should learn it is gating nothing, and a
/// package that never opted in must not fail for it.
pub(crate) fn attest_all(
    home: &home::Home,
    manifest: &Manifest,
    packages_resolved: &packages::ResolvedPackages,
) -> ExitCode {
    let docs = spec_scan_roots(manifest);
    let scanned = match claims::scan(&home.dir, &docs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The CI gate is a gate on claims. Backlog items are unbound by definition and muted from every
    // other reckoning — gating on them here would turn "I parked a bug in this doc" into a red
    // pipeline, which is the opposite of the point.
    let claims: Vec<&claims::Claim> =
        scanned.iter().filter(|c| c.kind == claims::Kind::Claim).collect();
    if claims.is_empty() {
        println!(
            "prova: attest — no claims declared here (no `[specs]` docs, or no `<!-- claim: id -->` anchors); nothing to gate on"
        );
        return ExitCode::SUCCESS;
    }
    let Some(recorded) = record::load(home) else {
        println!("prova: attest — no run has been recorded here; run the suite first (`prova`)");
        println!("  {} claim(s) declared, none attested", claims.len());
        return ExitCode::FAILURE;
    };
    let proofs = match collect_obligations(home, manifest, packages_resolved) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut attested = 0usize;
    println!("prova: attest — {} claim(s)", claims.len());
    for claim in &claims {
        let bindings: Vec<String> = proofs
            .iter()
            .filter(|p| p.covers.iter().any(|c| claims::split_pin(c).0 == claim.address))
            .map(|p| p.path.clone())
            .collect();
        let verdict = record::attest(&recorded, &bindings);
        match &verdict {
            record::Attested::Yes { path } => {
                attested += 1;
                println!("  ATTESTED  {}  — {path}", claim.address);
            }
            record::Attested::Red { path, .. } => {
                println!("  NOT       {}  — {path} did not pass", claim.address);
            }
            record::Attested::NoEvidence { path, why } => {
                println!("  NOT       {}  — {path} did not execute ({why})", claim.address);
            }
            record::Attested::Unbound => {
                println!("  NOT       {}  — no proof declares `covers` for it", claim.address);
            }
        }
    }
    println!("\n  {attested} of {} attested", claims.len());
    if attested == claims.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Resolve the project's inputs and compute the account in `prova_core::ledger`,
/// so the arithmetic is shared by the CLI, MCP, and any embedding host that depends on the core crate.
pub(crate) fn evidence_account(
    home: &home::Home,
    manifest: &Manifest,
    packages_resolved: &packages::ResolvedPackages,
) -> Result<prova_core::ledger::Account, String> {
    let docs = spec_scan_roots(manifest);
    let claims = claims::scan(&home.dir, &docs).map_err(|e| e.to_string())?;
    let proofs = collect_obligations(home, manifest, packages_resolved)?;
    // Tolerant on purpose, and `.ok()` is the whole of it: a missing record and an unreadable one
    // both mean "no evidence of a run", which the account already renders as an absence rather than
    // a zero. Propagating the error instead would make `prova evidence` fail outright on a corrupt
    // record — wrong for a verb whose contract is to report rather than gate, and inconsistent with
    // the DEPUTED section below, which reads the same file through `record::load` and tolerates it.
    let recorded = record::load(home);
    Ok(prova_core::ledger::account(&claims, &proofs, recorded.as_ref()))
}

/// `prova evidence` — the whole account: every stage of the obligation lifecycle with its count,
/// then the debts. The command the lifecycle was missing: `owed` shows only what is owed and
/// `attest` answers one address, so no verb could say where a project stands.
///
/// A report, never a gate — exit 0 belongs to the query family's contract, and the gate is
/// `prova attest`. Executes no proof body, like every query verb.
/// The console rendering of the account: the four-state header, the deputed and attention
/// sections (each absent entirely when nothing was adopted or declared, so a package that opted
/// into neither pays no output), and the owed tally.
fn print_evidence_console(home: &home::Home, account: prova_core::ledger::Account) {
    println!("prova: evidence for {}", home.dir.display());
    println!();
    println!("  CLAIMED   {:>4}   anchored claims in the declared docs", account.claimed);
    println!("  BOUND     {:>4}   covered by at least one proof", account.bound);
    println!("  PROMISED  {:>4}   proofs authored ahead of implementation", account.promised);
    match account.attested {
        Some(n) => println!(
            "  ATTESTED  {:>4}   covering proof executed and passed in the recorded run",
            n
        ),
        None => println!("  ATTESTED     —   no run recorded — run the suite first (`prova`)"),
    }

    // The deputed account (docs/design/verifiers.md): verdicts other verifiers produced,
    // conducted and adopted this run — counts only; `prova attest junit:<suite>#<name>` asks
    // about one. Absent entirely when nothing was ingested.
    let deputed = record::load(home).map(|r| r.deputed).unwrap_or_default();
    if !deputed.is_empty() {
        let red = deputed
            .iter()
            .filter(|d| d.outcome == "failed" || d.outcome == "error")
            .count();
        println!();
        println!(
            "  DEPUTED   {:>4}   cases adopted from other verifiers ({} red)",
            deputed.len(),
            red
        );
    }

    // The attention account rides along (docs/design/reminders.md): all three states, from the
    // record — `evidence` is a query verb and evaluates nothing. Absent entirely when the package
    // declares no reminders, so a package that adopted nothing pays no output for it.
    let reminders = record::load(home).map(|r| r.reminders).unwrap_or_default();
    if !reminders.is_empty() {
        let count = |s: &str| reminders.iter().filter(|e| e.state == s).count();
        println!();
        println!("  DUE       {:>4}   reminders owed attention (`prova reminders`)", count("due"));
        println!("  WATCHING  {:>4}   reminders armed — checked, the world holds still", count("watching"));
        let unevaluated = count("unevaluated");
        if unevaluated > 0 {
            println!("  UNEVAL    {:>4}   reminder conditions that could not run", unevaluated);
        }
    }

    let owed = account.owed;
    if owed.is_empty() {
        println!("\n  nothing owed");
    } else {
        use std::collections::BTreeMap;
        let mut by_status: BTreeMap<&'static str, usize> = BTreeMap::new();
        for o in &owed {
            *by_status.entry(o.status.tag()).or_default() += 1;
        }
        println!("\nowed:");
        for (tag, count) in by_status {
            println!("  {tag:<9} {count:>4}");
        }
        println!("  (`prova owed` lists each one; `prova attest` gates on the account)");
    }
}

pub(crate) fn evidence_subcommand(args: Vec<String>) -> ExitCode {
    let mut force_json = false;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if let Some(v) = value_flag(&arg, &mut it, &["--format"]) {
            match v.as_str() {
                "json" => force_json = true,
                "console" => {}
                other => {
                    eprintln!("prova evidence: unknown format {other:?} (expected console|json)");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova evidence [--format json]\n\n\
                     The whole account: CLAIMED / BOUND / PROMISED / ATTESTED with counts, then\n\
                     what is owed. `prova owed` lists each debt; `prova attest` gates on the account.\n\n\
                     --format json     emit the account as JSON (default: console)"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("prova: evidence: unexpected argument {other:?}\nusage: prova evidence [--format json]");
                return ExitCode::from(2);
            }
        }
    }
    let home = match resolve_home(None) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let (manifest, packages_resolved) = match resolve_for_obligations(&home) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let account = match evidence_account(&home, &manifest, &packages_resolved) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };

    if force_json {
        match serde_json::to_string(&account) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("prova: evidence: cannot serialize account: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    print_evidence_console(&home, account);
    ExitCode::SUCCESS
}

/// Collect every proof's `spec`/`covers` WITHOUT running anything. Reconciling prose against
/// proofs is a static question: it must not need a green suite, a docker daemon, or a broker.
///
/// **Takes the package's resolved plugins**, because collection LOADS every proof file and a proof
/// is entitled to `require("<a-local-plugin>")`. Building a bare `RunConfig` here — a second,
/// thinner slice of manifest resolution than a run uses — meant the first such `require` took the
/// whole ledger down with a Lua traceback. Every real package has local plugins, so `owed` and
/// `attest` were unusable on exactly the projects they are for.
pub(crate) fn collect_obligations(
    home: &home::Home,
    manifest: &Manifest,
    packages_resolved: &packages::ResolvedPackages,
) -> Result<Vec<prova_core::ProofObligation>, String> {
    let resolved = manifest.resolve(None)?;
    let config = engine_config(
        1,
        packages_resolved,
        Some(home),
        prova_core::progress::null(),
    );
    let mut out = Vec::new();
    for dir in find_proof_dirs(&home.dir, &resolved.proofs) {
        let suites = discover_suites(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for suite in suites {
            let found =
                prova_core::obligations_for_suite(suite.setup.as_deref(), &suite.files, &config)
                    .map_err(|e| e.to_string())?;
            out.extend(found);
        }
    }
    Ok(out)
}

/// Every proof file in the package — what `--pin` rewrites.
pub(crate) fn proof_files(home: &home::Home, manifest: &Manifest) -> Result<Vec<PathBuf>, String> {
    let resolved = manifest.resolve(None)?;
    let mut out = Vec::new();
    for dir in find_proof_dirs(&home.dir, &resolved.proofs) {
        let suites = discover_suites(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for suite in suites {
            out.extend(suite.files);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record's selection provenance: an EMPTY spelling is the load-bearing case (only an
    /// unselected run speaks for the whole suite); everything else names its subset in replayable
    /// flag form, exclusions included.
    #[test]
    fn spell_selection_names_the_subset_or_stays_empty() {
        let mut config = prova_core::RunConfig::default();
        assert!(spell_selection(&config).is_empty(), "a full run spells as nothing");
        config.selection.keywords.push("orders".into());
        config.selection.tag_excludes.push("slow".into());
        config.selection.nodes.push("a › b".into());
        let spelled = spell_selection(&config);
        assert_eq!(spelled, vec!["-k orders", "--tags !slow", "--node a › b"]);
    }

    /// A whole package in a tempdir, reconciled end to end: one anchored claim covered by a
    /// settled proof, one open promise — the account counts every lifecycle stage, and with no
    /// run recorded, ATTESTED is None (a stated fact), never a zero.
    #[test]
    fn evidence_account_reconciles_a_real_package() {
        let dir = std::env::temp_dir().join(format!("prova-evidence-ut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::create_dir_all(dir.join("proofs")).unwrap();
        std::fs::write(
            dir.join("prova.toml"),
            "[run]\nproofs = [\"proofs\"]\n\n[[specs.source]]\ntype = \"directory\"\npath = \"docs\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("docs/design.md"),
            "<!-- claim: never-preempt -->\nThe broker never preempts a lease.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("proofs/ledger_test.lua"),
            r#"
prova.test("covers the claim", { covers = "docs/design.md#never-preempt" }, function(t)
  t:expect(true):is_true()
end)
prova.test("promised work", { promises = "unit-test fixture" }, function(t)
  t:expect(false):is_true()
end)
"#,
        )
        .unwrap();

        let home = home::find(&dir).unwrap().expect("the tempdir package has a manifest");
        let (manifest, packages_resolved) =
            resolve_for_obligations(&home).map_err(|_| "resolve failed").unwrap();
        let account = evidence_account(&home, &manifest, &packages_resolved).unwrap();

        assert_eq!(account.claimed, 1);
        assert_eq!(account.bound, 1, "the covering proof binds the claim");
        assert_eq!(account.promised, 1, "the open promise is owed");
        assert_eq!(account.attested, None, "no record — a stated fact, never a zero");
        assert!(
            account.owed.iter().any(|o| o.subject.contains("promised work")),
            "the open promise appears in the owed rows: {:?}",
            account.owed
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
