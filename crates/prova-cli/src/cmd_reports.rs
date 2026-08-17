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

/// The forms a PERSON opens, best first. The picker offers these and nothing else: `json` and `xml`
/// are what an agent reads and what `--kind` addresses, and putting them in a human's menu is
/// offering to open a file they did not want to look at.
const HUMAN_KINDS: &[&str] = &["html", "md", "txt", "text"];

/// The form a person would want from this report, if any.
fn human_form(report: &prova_core::ledger::ReportRow) -> Option<(&str, &str)> {
    HUMAN_KINDS
        .iter()
        .find_map(|k| report.forms.get(*k).map(|p| (*k, p.as_str())))
}

/// Hand a path to the desktop's opener, without waiting for it.
///
/// SPAWNED, never waited on: `xdg-open` may hand off to a terminal browser that runs until the
/// reader quits, and prova blocking until someone closes a coverage report would be its own kind of
/// trap. The path is printed either way, so a silent opener failure still leaves the reader with
/// something to click.
fn open_path(path: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    // Unverified on Windows, like every other Windows path in this tree — `start` is a shell
    // builtin, hence the `cmd /C`, and the empty title argument is `start`'s quoting quirk.
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", path]);
        c
    };
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Render the whole account, or one report, from the last run's record.
pub(crate) fn reports_subcommand(args: Vec<String>) -> ExitCode {
    let mut name: Option<String> = None;
    let mut kind: Option<String> = None;
    let mut pick = false;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova reports [<name>] [--kind <kind>] [-c|--choose]\n\n\
                     The report account: every artifact the last run published, with the one line\n\
                     that says what it shows and every form it is available in.\n\n\
                     Bare, it LISTS what exists — the discovery mode. With a <name> it addresses one\n\
                     report and prints its forms and paths. With `--kind` it prints that form's path\n\
                     ALONE, so it composes:\n\n\
                     \x20 open $(prova reports coverage --kind html)\n\
                     \x20 jq . \"$(prova reports coverage --kind json)\"\n\n\
                     `-c` / `--choose` is the human lane: pick from a menu of the reports a person\n\
                     would open (html and friends) and it opens automatically; with a <name> it\n\
                     opens that one directly. It needs a terminal — anywhere else (a pipe, CI, an\n\
                     agent) it prints the listing instead of prompting, so it can never block.\n\n\
                     Artifacts are rendered by the tool that produced them and copied into custody\n\
                     under .prova/var/reports, so they outlive the conduct and the sweeping of\n\
                     target/. Prova preserves and addresses them; it does not draw them.\n\n\
                     Reads the last run's record — a report exists once a run has published it."
                );
                return ExitCode::SUCCESS;
            }
            "-c" | "--choose" => pick = true,
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
    if pick && kind.is_some() {
        eprintln!("prova: reports: --choose picks a form for you — drop --kind, or drop --choose");
        return ExitCode::from(2);
    }
    match (name, pick) {
        // `--choose` with a name: the reader already knows which report they want, so skip the menu
        // and open it. Same guard applies — see `open_named`.
        (Some(name), true) => open_named(&home, &name),
        (Some(name), false) => one(&home, &name, kind.as_deref()),
        (None, true) => choose(&home),
        (None, false) => list(&home),
    }
}

/// `prova reports <name> --choose` — open that report's human form directly.
///
/// No prompt here, so no terminal is needed to CHOOSE — but opening a browser from a pipeline is
/// still not what a program wants, so a non-terminal stdout falls back to printing the path. That
/// keeps the flag safe to acquire by accident in either spelling.
fn open_named(home: &Home, name: &str) -> ExitCode {
    use std::io::IsTerminal;
    let reports = match rows(home) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let Some(report) = reports.iter().find(|r| r.name == name) else {
        let known: Vec<&str> = reports.iter().map(|r| r.name.as_str()).collect();
        eprintln!("prova: no report named {name:?} — the last run published: {}", known.join(", "));
        return ExitCode::from(2);
    };
    let Some((kind, path)) = human_form(report) else {
        eprintln!(
            "prova: report {name:?} has no form a person would open (looked for: {}) —              address a machine form with `prova reports {name} --kind <kind>`",
            HUMAN_KINDS.join(", ")
        );
        return ExitCode::from(2);
    };
    if !std::io::stdout().is_terminal() {
        eprintln!("prova: reports --choose: stdout is not a terminal — printing the path instead");
        println!("{path}");
        return ExitCode::SUCCESS;
    }
    open_pick(name, kind, path)
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

/// One row in the picker: what it is, and what opening it will show.
struct Pick {
    name: String,
    summary: String,
    kind: String,
    path: String,
}

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}  —  {} [{}]", self.name, self.summary, self.kind)
    }
}

/// `--choose`: pick a report from a menu and open it.
///
/// **This must never become a trap for an agent, and the flag alone is not enough of a guard.** An
/// agent that lifts `prova reports -c` from a doc into a pipeline must get output, not a prompt it
/// cannot answer and cannot escape. So the prompt is gated on BOTH streams being a terminal — stdin
/// because a menu needs someone to drive it, stdout because a captured stdout is precisely the
/// shape of a program reading this rather than a person.
///
/// When that does not hold it **falls back to the plain listing** rather than erroring. `--choose`
/// asks the same question as a bare `prova reports` and only differs in presentation, so a context
/// that cannot present it should still answer it — and stdout keeps the same shape, so a script
/// that acquired the flag by accident is unharmed. The explanation goes to stderr, where it cannot
/// pollute what a reader parses.
fn choose(home: &Home) -> ExitCode {
    use std::io::IsTerminal;

    let reports = match rows(home) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let picks: Vec<Pick> = reports
        .iter()
        .filter_map(|r| {
            human_form(r).map(|(kind, path)| Pick {
                name: r.name.clone(),
                summary: r.summary.clone(),
                kind: kind.to_string(),
                path: path.to_string(),
            })
        })
        .collect();

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "prova: reports --choose needs a terminal to present a menu on — listing instead \
             (address one directly with `prova reports <name> --kind <kind>`)"
        );
        return list(home);
    }

    if picks.is_empty() {
        // Distinguish "nothing published" from "nothing a person would open": the second has a
        // real remedy — the machine-readable forms are still there and still addressable.
        if reports.is_empty() {
            return list(home);
        }
        println!(
            "prova: the last run published no report a person would open (looked for: {})\n  \
             machine-readable forms are still there — `prova reports` lists them",
            HUMAN_KINDS.join(", ")
        );
        return ExitCode::SUCCESS;
    }

    match inquire::Select::new("Open a report:", picks).prompt() {
        Ok(pick) => open_pick(&pick.name, &pick.kind, &pick.path),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => {
            eprintln!("prova: reports: cancelled");
            ExitCode::from(130)
        }
        Err(err) => {
            eprintln!("prova: reports: selection failed: {err}");
            ExitCode::from(2)
        }
    }
}

/// Open one chosen report, printing the path regardless so a failed opener never strands a reader.
fn open_pick(name: &str, kind: &str, path: &str) -> ExitCode {
    match open_path(path) {
        Ok(()) => {
            println!("prova: opening {name} ({kind})\n  {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("prova: could not open {path}: {e}");
            println!("{path}");
            ExitCode::from(2)
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
