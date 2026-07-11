//! `prova` CLI (POC).
//!
//! Usage:
//!   prova <file.lua>                 run a file (human console output)
//!   prova --format json <file.lua>   stream JSONL events (machine/GUI protocol)
//!   prova --list <file.lua>          discover tests without running them

use std::path::Path;
use std::process::ExitCode;

use prova_core::{discover_path, run_path, ConsoleReporter, JsonReporter, MultiReporter, Reporter};

enum Format {
    Console,
    Json,
}

fn main() -> ExitCode {
    let mut format = Format::Console;
    let mut list = false;
    let mut file: Option<String> = None;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--list" => list = true,
            "--format=json" => format = Format::Json,
            "--format=console" => format = Format::Console,
            "--json" => format = Format::Json,
            other if other.starts_with("--") => {
                eprintln!("prova: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => file = Some(other.to_string()),
        }
    }

    let Some(file) = file else {
        eprintln!("usage: prova [--list] [--format json] <file.lua>");
        return ExitCode::from(2);
    };
    let path = Path::new(&file);

    if list {
        return match discover_path(path) {
            Ok(paths) => {
                for p in paths {
                    println!("{p}");
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("prova: {err}");
                ExitCode::from(2)
            }
        };
    }

    let mut reporter: Box<dyn Reporter> = match format {
        Format::Console => Box::new(ConsoleReporter),
        Format::Json => Box::new(MultiReporter::new(vec![Box::new(JsonReporter::new(
            std::io::stdout(),
        ))])),
    };

    match run_path(path, reporter.as_mut()) {
        Ok(summary) if summary.is_success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}
