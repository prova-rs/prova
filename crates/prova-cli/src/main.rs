//! `prova` CLI (POC).
//!
//! Usage:
//!   prova <file.lua>                 run a file (human console output)
//!   prova --format json <file.lua>   stream JSONL events (machine/GUI protocol)
//!   prova --list <file.lua>          discover tests without running them
//!   prova --jobs N <file.lua>        run up to N units concurrently (throughput only)

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use prova_core::{
    discover_files, discover_path, run_suite, ConsoleReporter, JsonReporter, MultiReporter,
    Reporter, RunConfig,
};

enum Format {
    Console,
    Json,
}

fn main() -> ExitCode {
    let mut format = Format::Console;
    let mut list = false;
    let mut jobs: usize = 1;
    let mut paths: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        // `--jobs`/`-j` takes a value, either `--jobs N` or `--jobs=N`.
        let jobs_value = arg
            .strip_prefix("--jobs=")
            .or_else(|| arg.strip_prefix("-j="))
            .map(str::to_string)
            .or_else(|| (arg == "--jobs" || arg == "-j").then(|| args.next().unwrap_or_default()));
        if let Some(value) = jobs_value {
            match value.parse::<usize>() {
                Ok(n) if n >= 1 => jobs = n,
                _ => {
                    eprintln!("prova: --jobs expects a positive integer, got {value:?}");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        match arg.as_str() {
            "--list" => list = true,
            "--format=json" => format = Format::Json,
            "--format=console" => format = Format::Console,
            "--json" => format = Format::Json,
            other if other.starts_with('-') => {
                eprintln!("prova: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => paths.push(other.to_string()),
        }
    }

    if paths.is_empty() {
        eprintln!("usage: prova [--list] [--format json] [--jobs N] <file-or-dir>...");
        return ExitCode::from(2);
    }

    // Expand each argument (a file or a directory) into concrete test files.
    let mut files: Vec<PathBuf> = Vec::new();
    for arg in &paths {
        match discover_files(Path::new(arg)) {
            Ok(found) => files.extend(found),
            Err(err) => {
                eprintln!("prova: {arg}: {err}");
                return ExitCode::from(2);
            }
        }
    }
    if files.is_empty() {
        eprintln!("prova: no test files found (looked for *_test.lua / *.test.lua)");
        return ExitCode::from(2);
    }

    if list {
        for file in &files {
            match discover_path(file) {
                Ok(node_paths) => {
                    for p in node_paths {
                        println!("{p}");
                    }
                }
                Err(err) => {
                    eprintln!("prova: {}: {err}", file.display());
                    return ExitCode::from(2);
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    let mut reporter: Box<dyn Reporter> = match format {
        Format::Console => Box::new(ConsoleReporter),
        Format::Json => Box::new(MultiReporter::new(vec![Box::new(JsonReporter::new(
            std::io::stdout(),
        ))])),
    };

    let config = RunConfig { concurrency: jobs };
    match run_suite(&files, reporter.as_mut(), &config) {
        Ok(summary) if summary.is_success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}
