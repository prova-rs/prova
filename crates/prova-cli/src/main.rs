//! `prova` CLI (POC). Usage: `prova <file.lua>` — runs a single test file and reports.

use std::path::Path;
use std::process::ExitCode;

use prova_core::{run_path, ConsoleReporter};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: prova <file.lua>");
        return ExitCode::from(2);
    };

    let mut reporter = ConsoleReporter;
    match run_path(Path::new(&path), &mut reporter) {
        Ok(summary) if summary.is_success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}
