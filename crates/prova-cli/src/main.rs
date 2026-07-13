//! `prova` CLI.
//!
//! Usage:
//!   prova <file-or-dir>...           run the given files/dirs (console output)
//!   prova                            run the suite declared in ./prova.toml
//!   prova --profile ci               run the `ci` profile from ./prova.toml
//!   prova --manifest path.toml       use a specific manifest
//!   prova --format json <path>       stream JSONL events (machine/GUI protocol)
//!   prova --list <path>              discover tests without running them
//!   prova --jobs N <path>            run up to N units concurrently (throughput only)
//!
//! CLI flags override manifest values; explicit path arguments bypass the manifest entirely.

mod manifest;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use manifest::{Manifest, SuiteDecl};
use prova_core::{
    discover_files, discover_path, discover_suites, run_suites, ConsoleReporter, JsonReporter,
    MultiReporter, Reporter, RunConfig, Suite,
};

const HELP: &str = "\
usage:
  prova <file-or-dir>...    run the given files/dirs
  prova                     run the suite declared in ./prova.toml

options:
  -p, --profile NAME        run a profile from the manifest
      --manifest PATH       use a specific manifest (default ./prova.toml)
      --format console|json output format (--json is shorthand)
  -j, --jobs N              run up to N units concurrently
      --list                discover tests without running them
  -V, --version             print version
  -h, --help                print this help";

enum Format {
    Console,
    Json,
}

/// Match `--name value` / `--name=value` (and any aliases); returns the value if `arg` is one.
fn value_flag(
    arg: &str,
    args: &mut impl Iterator<Item = String>,
    names: &[&str],
) -> Option<String> {
    for name in names {
        if let Some(v) = arg.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
        if arg == *name {
            return Some(args.next().unwrap_or_default());
        }
    }
    None
}

fn main() -> ExitCode {
    let mut cli_format: Option<Format> = None;
    let mut cli_jobs: Option<usize> = None;
    let mut list = false;
    let mut explicit_paths: Vec<String> = Vec::new();
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(v) = value_flag(&arg, &mut args, &["--jobs", "-j"]) {
            match v.parse::<usize>() {
                Ok(n) if n >= 1 => cli_jobs = Some(n),
                _ => {
                    eprintln!("prova: --jobs expects a positive integer, got {v:?}");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut args, &["--profile", "-p"]) {
            profile = Some(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut args, &["--manifest"]) {
            manifest_path = Some(v);
            continue;
        }
        // `--format json` and `--format=json` both work.
        if let Some(v) = value_flag(&arg, &mut args, &["--format"]) {
            match v.as_str() {
                "json" => cli_format = Some(Format::Json),
                "console" => cli_format = Some(Format::Console),
                other => {
                    eprintln!("prova: unknown format {other:?} (expected console|json)");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        match arg.as_str() {
            "--list" => list = true,
            "--json" => cli_format = Some(Format::Json),
            "--version" | "-V" => {
                println!("prova {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("prova: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => explicit_paths.push(other.to_string()),
        }
    }

    // Resolve the run: explicit path args bypass the manifest; otherwise read prova.toml.
    let (paths, jobs, format, declared) = if !explicit_paths.is_empty() {
        (
            explicit_paths,
            cli_jobs.unwrap_or(1),
            cli_format.unwrap_or(Format::Console),
            BTreeMap::new(),
        )
    } else {
        match resolve_from_manifest(manifest_path, profile, cli_jobs, cli_format) {
            Ok(resolved) => resolved,
            Err(code) => return code,
        }
    };

    // Build the suites to run: first any explicit `[suites.*]` from the manifest (each groups its
    // discovered files under one name + optional setup), then the plain paths — a directory with a
    // `suite.lua` is one suite (files share a state → shared `Scope.Suite`), every other file a
    // singleton. `--jobs` parallelizes across suites.
    let mut suites: Vec<Suite> = Vec::new();
    for (name, decl) in &declared {
        let mut files = Vec::new();
        for p in &decl.paths {
            match discover_files(Path::new(p)) {
                Ok(found) => files.extend(found),
                Err(err) => {
                    eprintln!("prova: suite {name:?}: {p}: {err}");
                    return ExitCode::from(2);
                }
            }
        }
        files.sort();
        if !files.is_empty() {
            suites.push(Suite {
                name: name.clone(),
                setup: decl.setup.as_ref().map(PathBuf::from),
                files,
            });
        }
    }
    for arg in &paths {
        match discover_suites(Path::new(arg)) {
            Ok(found) => suites.extend(found),
            Err(err) => {
                eprintln!("prova: {arg}: {err}");
                return ExitCode::from(2);
            }
        }
    }
    if suites.is_empty() {
        eprintln!("prova: no test files found (looked for *_test.lua / *.test.lua)");
        return ExitCode::from(2);
    }

    if list {
        for file in suites.iter().flat_map(|s| &s.files) {
            match discover_path(file) {
                Ok(node_paths) => node_paths.iter().for_each(|p| println!("{p}")),
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

    // The standalone `prova` binary ships the archetect plugin, so `archetect.render{...}` works.
    let config = RunConfig::new(jobs).with_module(prova_archetect::install);
    match run_suites(&suites, reporter.as_mut(), &config) {
        Ok(summary) if summary.is_success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}

/// Read `prova.toml` (or `--manifest`), overlay `--profile`, apply env, and merge CLI overrides.
/// Returns (paths, jobs, format, declared-suites) or an exit code on error.
#[allow(clippy::type_complexity)]
fn resolve_from_manifest(
    manifest_path: Option<String>,
    profile: Option<String>,
    cli_jobs: Option<usize>,
    cli_format: Option<Format>,
) -> Result<(Vec<String>, usize, Format, BTreeMap<String, SuiteDecl>), ExitCode> {
    let explicit_manifest = manifest_path.is_some();
    let path = manifest_path.unwrap_or_else(|| "prova.toml".to_string());

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            if explicit_manifest || profile.is_some() {
                eprintln!("prova: cannot read manifest {path:?}");
            } else {
                eprintln!(
                    "usage: prova <file-or-dir>...   or   prova [--profile NAME]  (reads prova.toml)"
                );
            }
            return Err(ExitCode::from(2));
        }
    };

    let manifest = Manifest::parse(&text).map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;
    let resolved = manifest.resolve(profile.as_deref()).map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;
    if resolved.paths.is_empty() && resolved.suites.is_empty() {
        eprintln!("prova: manifest {path:?} defines no paths or suites to run");
        return Err(ExitCode::from(2));
    }

    // Apply the run environment before tests execute.
    for (key, value) in &resolved.env {
        std::env::set_var(key, value);
    }

    let jobs = cli_jobs.or(resolved.jobs).unwrap_or(1);
    let format = match cli_format {
        Some(f) => f,
        None => match resolved.format.as_deref() {
            Some("json") => Format::Json,
            None | Some("console") => Format::Console,
            Some(other) => {
                eprintln!("prova: unknown format {other:?} in manifest (expected console|json)");
                return Err(ExitCode::from(2));
            }
        },
    };
    Ok((resolved.paths, jobs, format, resolved.suites))
}
