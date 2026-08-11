//! The spec-lane verbs: burndown, owed, promote, tests, specs.

use super::*;

/// `prova tests burndown` — the implementing inner loop: `--promises --due`, so open promises fail
/// loud with full detail and kept promises demand graduation. `--allow-empty` rides along because an
/// empty surface here means the burndown is COMPLETE — exit 0, not a selection error. (The
/// tests-lane driver; the retired top-level `prova burndown` is gone.)
pub(crate) fn burndown_subcommand(args: Vec<String>) -> ExitCode {
    let mut full = vec![
        "--promises".to_string(),
        "--due".to_string(),
        "--allow-empty".to_string(),
    ];
    full.extend(args);
    run(full)
}

/// The spec scan roots for the obligation verbs — one door, so the resolution cannot drift between
/// `owed`/`attest`/`evidence`/`backlog`, and the `[specs] docs` deprecation is announced exactly
/// once per invocation. No `[specs]` (or an empty one) yields no roots: completely opt-in.
pub(crate) fn spec_scan_roots(manifest: &Manifest) -> Vec<String> {
    let Some(specs) = manifest.specs.as_ref() else {
        return Vec::new();
    };
    if specs.uses_deprecated_docs() {
        eprintln!(
            "prova: `[specs] docs = [...]` is deprecated — use `[[specs.source]]` with \
             `type = \"directory\"` (see `prova learn spec`)"
        );
    }
    specs.scan_roots()
}

/// `prova owed` — the obligation ledger: everything this package owes, from every origin.
///
/// An agent orienting in a repo should ask ONE question — what is owed here? — so open promises
/// (in-repo deferrals) and unproven claims (obligations that entered from prose) land in one list
/// with origin as a column. An answer that lives in two places has one that goes stale.
///
/// Reports, never gates. The single exception is a duplicate claim id, which makes an address
/// ambiguous and is a defect rather than unfinished work.
pub(crate) fn owed_subcommand(args: Vec<String>) -> ExitCode {
    let pinning = args.iter().any(|a| a == "--pin");
    let home = match resolve_home(None) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let (manifest, packages_resolved) = match resolve_for_obligations(&home) {
        Ok(pair) => pair,
        Err(code) => return code,
    };

    // The manifest entry IS the opt-in. No `[specs]` means no scan, no cost, and no lecture about
    // a subsystem this package never asked for — but open promises are still owed, so the ledger still
    // has something to say.
    let docs = spec_scan_roots(&manifest);

    let claims = match claims::scan(&home.dir, &docs) {
        Ok(claims) => claims,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };

    let proofs = match collect_obligations(&home, &manifest, &packages_resolved) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };

    if pinning {
        let files = match proof_files(&home, &manifest) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::FAILURE;
            }
        };
        match claims::pin(&files, &claims) {
            Ok(n) => println!("prova: pinned claim text in {n} file(s)"),
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let owed = claims::reconcile(&claims, &proofs);

    // DUE reminders join the narrowing (docs/design/reminders.md): an arriving agent asks ONE
    // question — what is owed here? — and attention owed is part of the answer. Read from the run
    // record, never evaluated: `owed` is a query verb and executes nothing. Watching and
    // unevaluated reminders live in `prova reminders`, not here — only DUE is *owed*.
    let due: Vec<record::ReminderEntry> = record::load(&home)
        .map(|r| r.reminders.into_iter().filter(|e| e.is_due()).collect())
        .unwrap_or_default();

    if owed.is_empty() && due.is_empty() {
        println!("prova: nothing owed — no open promises, every claim is covered, and no reminder is due");
        return ExitCode::SUCCESS;
    }
    for row in &owed {
        println!("  {:<9} {}", row.status.tag(), row.subject);
        println!("            {}", row.detail);
    }
    for e in &due {
        println!("  {:<9} {}", "DUE", e.name);
        match &e.why {
            Some(w) => println!("            {w} — {}", e.message),
            None => println!("            {}", e.message),
        }
    }
    println!();
    println!("  {} owed", owed.len() + due.len());
    ExitCode::SUCCESS
}

/// The specs lane's one state-transition write: thaw a backlog item into a claim, in place (a
/// keyword flip — the id and prose stay put, only the state changes). Shared by `prova specs promote
/// <id>` (the lane driver) and, transitionally, `prova backlog promote <id>`. Demotion is
/// deliberately not a CLI verb: cooling a claim back is only safe when nothing binds it, a check that
/// needs the proofs in hand.
pub(crate) fn promote_claim(id: Option<String>) -> ExitCode {
    let Some(id) = id else {
        eprintln!("prova: promote <id> — which backlog item?\nusage: prova specs promote <doc.md#id | id>");
        return ExitCode::from(2);
    };
    let home = match resolve_home(None) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let manifest = match read_manifest(&home) {
        Ok(m) => m,
        Err(code) => return code,
    };
    let docs = spec_scan_roots(&manifest);
    let scanned = match claims::scan(&home.dir, &docs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Resolve by full address when the arg carries a `#`, else by bare id — the courtesy `attest`
    // extends, since a human has the id but not always the path.
    let candidates: Vec<&claims::Claim> = if id.contains('#') {
        scanned.iter().filter(|c| c.address == id).collect()
    } else {
        claims::matching_id(&scanned, &id)
    };
    let target = match candidates.as_slice() {
        [one] => *one,
        [] => {
            eprintln!("prova: no backlog item or claim with id {id:?}");
            return ExitCode::FAILURE;
        }
        many => {
            eprintln!("prova: ambiguous — {} anchors carry {id:?}:", many.len());
            for m in many {
                eprintln!("    {}", m.address);
            }
            eprintln!("  name the full address to disambiguate");
            return ExitCode::from(2);
        }
    };
    if target.kind == claims::Kind::Claim {
        println!("prova: {} is already a claim — nothing to promote", target.address);
        return ExitCode::SUCCESS;
    }
    match claims::promote(target, &home.dir) {
        Ok(()) => {
            println!("prova: promoted {} — backlog → claim", target.address);
            println!("  it is now owed; write a proof that `covers = \"{}\"` to discharge it", target.address);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("prova: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `prova tests` — the tests lane: every discovered node, state-tagged PROMISE (open, authored ahead
/// of implementation) or PROOF (settled). A query verb; runs nothing. `--promises` / `--proofs`
/// narrow to one state. This is the lane report `prova list` stood in for; `list` retires in a later
/// increment (docs/plans/query-consolidation.md).
pub(crate) fn tests_subcommand(args: Vec<String>) -> ExitCode {
    // Drivers: `prova tests <driver>` acts on the lane. burndown/falsify are red→green worklists that
    // delegate to the run engine. Any other first arg (a `--flag`, or a file/dir path) falls through
    // to the report — `prova tests`, `prova tests --promises`, `prova tests path/to/dir`.
    match args.split_first() {
        Some((first, rest)) if first == "burndown" => burndown_subcommand(rest.to_vec()),
        Some((first, rest)) if first == "falsify" => falsify_subcommand(rest.to_vec()),
        _ => {
            let mut full = vec!["--list-tagged".to_string()];
            full.extend(args);
            run(full)
        }
    }
}

/// `prova specs` — the specs lane: every claim and backlog item in the `[specs]` docs, each
/// state-tagged. The lane report that unifies what `prova backlog` (cold shelf) and the obligation
/// family (`owed`/`attest`) each saw only one slice of — CLAIM and BACKLOG side by side. A query
/// verb: it reads anchors and reports, gating nothing. `--claims` / `--backlog` narrow to one
/// state (docs/plans/query-consolidation.md). Selectors (`-k`/`--tags`) arrive with the shared
/// query engine in a later increment.
/// Parse the specs lane's filters: the state (`--claims` xor `--backlog`) and `--undated` —
/// only items carrying no `YYYY-MM-DD` draw-down date (a date is what lets a reminder draw an
/// item down); it composes with the state filter. `Err(ExitCode::SUCCESS)` is `--help`.
fn parse_specs_args(args: &[String]) -> Result<(Option<claims::Kind>, bool), ExitCode> {
    let mut want: Option<claims::Kind> = None;
    let mut undated_only = false;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova specs                 list the specs lane: every claim and backlog item\n\
                     \x20      prova specs --claims        only the claims (owed obligations)\n\
                     \x20      prova specs --backlog       only the backlog (captured, not yet owed)\n\
                     \x20      prova specs --undated       only items with no draw-down date (composes)\n\
                     \x20      prova specs promote <id>    thaw a backlog item into a claim, in place\n\
                     \x20      prova specs backfill        proofs no claim backs — the reverse of `owed`\n\n\
                     Claims and backlog are the two states of one prose obligation, sharing the\n\
                     `[specs]` sources. Promotion is a keyword flip: the id and prose stay put.\n\
                     Backfill gates (exit non-zero) while any proof is unbacked — a worklist to drive down."
                );
                return Err(ExitCode::SUCCESS);
            }
            "--claims" | "--claim" => {
                if want == Some(claims::Kind::Backlog) {
                    eprintln!("prova: specs: --claims and --backlog are mutually exclusive");
                    return Err(ExitCode::from(2));
                }
                want = Some(claims::Kind::Claim);
            }
            "--backlog" => {
                if want == Some(claims::Kind::Claim) {
                    eprintln!("prova: specs: --claims and --backlog are mutually exclusive");
                    return Err(ExitCode::from(2));
                }
                want = Some(claims::Kind::Backlog);
            }
            "--undated" => undated_only = true,
            other => {
                eprintln!("prova: specs: unexpected argument {other:?}\nusage: prova specs [--claims | --backlog] [--undated]");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok((want, undated_only))
}

pub(crate) fn specs_subcommand(args: Vec<String>) -> ExitCode {
    // Drivers on the specs lane. `promote <id>` thaws a backlog item into a claim (the one
    // state-write, shared with `prova backlog promote`). `backfill` is the reverse-`owed` worklist:
    // proofs no claim backs — a red→green gate the agent drives by writing the missing specs. It
    // routes through the run/discover machinery (it reads the tests lane's `covers`), so it delegates
    // to `run(--backfill)` rather than the `[specs]` scan the report below uses.
    if let Some((first, rest)) = args.split_first() {
        if first == "promote" {
            return promote_claim(rest.first().cloned());
        }
        if first == "backfill" {
            let mut full = vec!["--backfill".to_string()];
            full.extend(rest.iter().cloned());
            return run(full);
        }
    }
    let (want, undated_only) = match parse_specs_args(&args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let home = match resolve_home(None) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let manifest = match read_manifest(&home) {
        Ok(m) => m,
        Err(code) => return code,
    };
    let docs = spec_scan_roots(&manifest);

    // No spec source at all: invoking `prova specs` IS the signal you want the feature, so a pointer
    // to how to declare one is help, not a lecture (the same courtesy `prova backlog` extends).
    if docs.is_empty() {
        println!("prova: no spec source configured — prova does not know where your claims and backlog items live.");
        println!("  Declare one in prova.toml:");
        println!();
        println!("      [[specs.source]]");
        println!("      type = \"directory\"");
        println!("      path = \"docs\"");
        println!();
        println!("  then `prova specs` lists them. See `prova learn spec`.");
        return ExitCode::SUCCESS;
    }

    let scanned = match claims::scan(&home.dir, &docs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::FAILURE;
        }
    };
    let items: Vec<&claims::Claim> = scanned
        .iter()
        .filter(|c| match &want {
            Some(k) => c.kind == *k,
            None => true,
        })
        .filter(|c| !undated_only || c.date.is_none())
        .collect();

    if items.is_empty() {
        match (want, undated_only) {
            (_, true) => {
                println!("prova: nothing undated — every matching item carries a draw-down date")
            }
            (Some(claims::Kind::Claim), _) => {
                println!("prova: no claims — no `<!-- claim: id -->` anchors in the spec sources")
            }
            (Some(claims::Kind::Backlog), _) => {
                println!("prova: nothing in the backlog — no `<!-- backlog: id -->` anchors in the spec sources")
            }
            (None, _) => {
                println!("prova: the specs lane is empty — no claim or backlog anchors in the spec sources")
            }
        }
        return ExitCode::SUCCESS;
    }

    for c in &items {
        let tag = match c.kind {
            claims::Kind::Claim => "CLAIM",
            claims::Kind::Backlog => "BACKLOG",
        };
        let loc = format!("{}:{}", c.file.display(), c.line);
        match &c.date {
            Some(d) => println!("  {tag:<8} {:<40} {loc}  ({d})", c.address),
            None => println!("  {tag:<8} {:<40} {loc}", c.address),
        }
    }
    let claim_n = items.iter().filter(|c| c.kind == claims::Kind::Claim).count();
    let backlog_n = items.len() - claim_n;
    let undated_n = items.iter().filter(|c| c.date.is_none()).count();
    println!();
    let head = match want {
        Some(claims::Kind::Claim) => format!("{claim_n} claim(s)"),
        Some(claims::Kind::Backlog) => format!("{backlog_n} in backlog"),
        None => format!("{claim_n} claim(s), {backlog_n} in backlog"),
    };
    // Nudge toward dating — a date is what lets a reminder draw an item down by its deadline. Skip
    // the nudge when already filtered to the undated ones (there the count IS the list).
    if !undated_only && undated_n > 0 {
        println!("  {head} · {undated_n} undated (`prova specs --undated`)");
    } else {
        println!("  {head}");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The lane's filters: state selection is exclusive in BOTH orders, `--undated` composes,
    /// and anything unexpected is a usage error.
    #[test]
    fn specs_filters_compose_and_exclude() {
        assert_eq!(parse_specs_args(&args(&[])).unwrap(), (None, false));
        assert_eq!(
            parse_specs_args(&args(&["--claims", "--undated"])).unwrap(),
            (Some(claims::Kind::Claim), true)
        );
        assert_eq!(
            parse_specs_args(&args(&["--backlog"])).unwrap(),
            (Some(claims::Kind::Backlog), false)
        );
        assert!(parse_specs_args(&args(&["--claims", "--backlog"])).is_err());
        assert!(parse_specs_args(&args(&["--backlog", "--claims"])).is_err(), "exclusive both ways");
        assert!(parse_specs_args(&args(&["stray"])).is_err());
    }

    /// The one door to spec scan roots: no `[specs]` (or an empty one) opts out entirely; each
    /// `[[specs.source]]` directory contributes its path.
    #[test]
    fn spec_scan_roots_are_opt_in() {
        let m = Manifest::parse("[run]\n").unwrap();
        assert!(spec_scan_roots(&m).is_empty(), "no [specs] means no scan");
        let m = Manifest::parse(
            "[[specs.source]]\ntype = \"directory\"\npath = \"docs\"\n\
             [[specs.source]]\ntype = \"directory\"\npath = \"notes\"\n",
        )
        .unwrap();
        assert_eq!(spec_scan_roots(&m), vec!["docs".to_string(), "notes".to_string()]);
    }

    /// The specs lane against a real package, end to end. ONE test so cwd changes once: nextest
    /// runs process-per-test, and every other test in this binary uses absolute paths.
    #[test]
    fn the_specs_lane_walks_a_real_package() {
        let code = |c: ExitCode| format!("{c:?}");
        let dir = std::env::temp_dir().join(format!("prova-specs-ut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(
            dir.join("prova.toml"),
            "[run]\nproofs = [\"proofs\"]\n\n[[specs.source]]\ntype = \"directory\"\npath = \"docs\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("docs/design.md"),
            "<!-- claim: never-preempt -->\nThe broker never preempts a lease.\n\n\
             <!-- backlog: lease-renewal -->\nLeases renew before expiry.\n",
        )
        .unwrap();
        std::env::set_current_dir(&dir).unwrap();

        // The report lists both states; the filters narrow it; all of it exits 0 (a query verb).
        for argv in [vec![], vec!["--claims".to_string()], vec!["--undated".to_string()]] {
            assert_eq!(code(specs_subcommand(argv)), code(ExitCode::SUCCESS));
        }

        // Promotion refusals: no id is usage, an unknown id is a failure naming it.
        assert_eq!(code(promote_claim(None)), code(ExitCode::from(2)));
        assert_eq!(code(promote_claim(Some("ghost".into()))), code(ExitCode::FAILURE));

        // The one state-write: backlog → claim is a keyword flip in place, id and prose intact.
        assert_eq!(code(promote_claim(Some("lease-renewal".into()))), code(ExitCode::SUCCESS));
        let doc = std::fs::read_to_string(dir.join("docs/design.md")).unwrap();
        assert!(doc.contains("<!-- claim: lease-renewal -->"), "flipped: {doc}");
        assert!(!doc.contains("backlog: lease-renewal"), "the old keyword is gone");
        assert!(doc.contains("Leases renew before expiry."), "prose stays put");

        // Promoting a claim again is a no-op courtesy, not an error.
        assert_eq!(code(promote_claim(Some("lease-renewal".into()))), code(ExitCode::SUCCESS));

        // `owed` reports the two uncovered claims and still exits 0 — reports never gate.
        assert_eq!(code(owed_subcommand(vec![])), code(ExitCode::SUCCESS));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
