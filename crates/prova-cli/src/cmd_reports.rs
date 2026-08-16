//! `prova reports` — the report account: what the last run produced, and where to read it.
//!
//! The custody half of the verifiers seam
//! (docs/design/verifiers.md#reports-are-custody-not-visualization). A conduct publishes an
//! artifact; this verb is how a reader finds it. Prova renders the one-line summary and the index —
//! counts and rows, the only visualization in bounds — and hands out paths to the artifacts the
//! DEPUTY rendered.
//!
//! Two modes, because discovery and addressing are different needs:
//!
//!   - **List** (`prova reports`): what exists, what each shows, and which forms it comes in. For a
//!     human who does not yet know what is available, and for an agent deciding what to open.
//!   - **Address** (`prova reports <name> [--kind K]`): one report, its forms and paths. With
//!     `--kind` it prints THE PATH ALONE, so it composes — `open $(prova reports coverage --kind
//!     html)` is the whole viewing story, and no platform-specific opener has to live in prova.

use std::process::ExitCode;

use crate::home::{self, Home};

/// Render the whole account, or one report, from the last run's record.
pub(crate) fn reports_subcommand(args: Vec<String>) -> ExitCode {
    let mut name: Option<String> = None;
    let mut kind: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova reports [<name>] [--kind <kind>]\n\n\
                     The report account: every artifact the last run published, with the one line\n\
                     that says what it shows and every form it is available in.\n\n\
                     Bare, it LISTS what exists — the discovery mode. With a <name> it addresses one\n\
                     report and prints its forms and paths. With `--kind` it prints that form's path\n\
                     ALONE, so it composes:\n\n\
                     \x20 open $(prova reports coverage --kind html)\n\
                     \x20 jq . \"$(prova reports coverage --kind json)\"\n\n\
                     Artifacts are rendered by the tool that produced them and copied into custody\n\
                     under .prova/var/reports, so they outlive the conduct and the sweeping of\n\
                     target/. Prova preserves and addresses them; it does not draw them.\n\n\
                     Reads the last run's record — a report exists once a run has published it."
                );
                return ExitCode::SUCCESS;
            }
            "--kind" => match it.next() {
                Some(k) => kind = Some(k),
                None => {
                    eprintln!("prova: reports: --kind needs a value (json, html, xml, …)");
                    return ExitCode::from(2);
                }
            },
            other if other.starts_with("--kind=") => {
                kind = Some(other["--kind=".len()..].to_string())
            }
            other if other.starts_with('-') => {
                eprintln!("prova: reports: unknown flag {other:?} (see --help)");
                return ExitCode::from(2);
            }
            other => name = Some(other.to_string()),
        }
    }

    let home = match home::find(std::path::Path::new(".")) {
        Ok(Some(h)) => h,
        Ok(None) => {
            eprintln!("prova: no prova.toml found — reports belong to a package (`prova init`)");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("prova: {e}");
            return ExitCode::from(2);
        }
    };
    // `--kind` addresses a form, so it needs to know WHICH report — otherwise the answer is
    // ambiguous the moment a second report exists.
    if kind.is_some() && name.is_none() {
        eprintln!("prova: reports: --kind addresses a form of ONE report — name it (`prova reports <name> --kind …`)");
        return ExitCode::from(2);
    }
    match name {
        Some(name) => one(&home, &name, kind.as_deref()),
        None => list(&home),
    }
}

/// The report rows the last run recorded, or a diagnosis of why there are none.
fn rows(home: &Home) -> Result<Vec<prova_core::ledger::ReportRow>, ExitCode> {
    match crate::record::load(home) {
        Some(record) => Ok(record.reports),
        // No record at all is a different situation from a record with no reports, and the remedy
        // differs: run something, versus publish something.
        None => {
            println!("prova: no run has been recorded yet — reports appear once a run publishes one");
            Err(ExitCode::SUCCESS)
        }
    }
}

/// Discovery: everything the last run published.
fn list(home: &Home) -> ExitCode {
    let reports = match rows(home) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if reports.is_empty() {
        println!(
            "prova: the last run published no reports\n  \
             a conduct publishes one with `report.publish{{ name, summary, forms }}` \
             (`prova learn verifiers`)"
        );
        return ExitCode::SUCCESS;
    }
    println!("prova: reports from the last run\n");
    for r in &reports {
        let kinds: Vec<&str> = r.forms.keys().map(String::as_str).collect();
        println!("  {}", r.name);
        println!("    {}", r.summary);
        println!("    forms: {}", kinds.join(", "));
        if !r.explains.is_empty() {
            println!("    explains: {}", r.explains.join(", "));
        }
        println!();
    }
    println!("  address one with `prova reports <name>`, or a form with `--kind <kind>`");
    ExitCode::SUCCESS
}

/// Addressing: one report, or one form of it.
fn one(home: &Home, name: &str, kind: Option<&str>) -> ExitCode {
    let reports = match rows(home) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let Some(report) = reports.iter().find(|r| r.name == name) else {
        // Name what DOES exist: the commonest reason to be here is a half-remembered name.
        let known: Vec<&str> = reports.iter().map(|r| r.name.as_str()).collect();
        eprintln!(
            "prova: no report named {name:?}{}",
            if known.is_empty() {
                " — the last run published none".to_string()
            } else {
                format!(" — the last run published: {}", known.join(", "))
            }
        );
        return ExitCode::from(2);
    };

    // `--kind`: the path alone, so it composes into `open $(…)`. Everything else goes to stderr so
    // stdout carries exactly the path and nothing to strip.
    if let Some(kind) = kind {
        match report.forms.get(kind) {
            Some(path) => {
                println!("{path}");
                return ExitCode::SUCCESS;
            }
            None => {
                let kinds: Vec<&str> = report.forms.keys().map(String::as_str).collect();
                eprintln!(
                    "prova: report {name:?} has no {kind:?} form — it comes in: {}",
                    kinds.join(", ")
                );
                return ExitCode::from(2);
            }
        }
    }

    println!("prova: report {}\n", report.name);
    println!("  {}", report.summary);
    if !report.explains.is_empty() {
        println!("\n  evidence for: {}", report.explains.join(", "));
    }
    println!("\n  forms:");
    for (kind, path) in &report.forms {
        println!("    {kind:<6} {path}");
    }
    println!("\n  a path alone: `prova reports {} --kind <kind>`", report.name);
    ExitCode::SUCCESS
}
