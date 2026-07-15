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
mod plugins;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use manifest::{Manifest, SuiteDecl};
use prova_core::{
    discover_files, discover_path_with, discover_suites, run_suites, ConsoleReporter, JsonReporter,
    MultiReporter, Reporter, RunConfig, Suite, SystemLayout, XdgSystemLayout,
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
  -P, --plugin name=source  add an ad-hoc plugin (repeatable; layers over the manifest)
      --list                discover tests without running them
  -V, --version             print version
  -h, --help                print this help";

/// The running prova version, checked against each plugin's `requires.prova` compatibility range.
const PROVA_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    // Subcommands: `prova plugin <...>`. Everything else is the run path (paths + flags).
    let mut raw = std::env::args().skip(1).peekable();
    if raw.peek().map(String::as_str) == Some("plugin") {
        raw.next();
        return plugin_subcommand(raw.collect());
    }

    run(std::env::args().skip(1).collect())
}

/// `prova plugin lint <file>...` — check each plugin file against the namespacing grammar.
fn plugin_subcommand(args: Vec<String>) -> ExitCode {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("lint") => {
            let files: Vec<String> = args.collect();
            if files.is_empty() {
                eprintln!("usage: prova plugin lint <file>...");
                return ExitCode::from(2);
            }
            let mut ok = true;
            for file in &files {
                // Lint loads each plugin with the same primitives + archetect module a run would
                // install, plus the plugin's own namespace so its intra-plugin `require`s resolve.
                let path = Path::new(file);
                let mut config = RunConfig::new(1).with_module(prova_archetect::install);
                if let Some((canonical, dir)) = plugins::namespace_for_file(path) {
                    config = config.with_plugin_namespace(canonical, dir);
                }
                match prova_core::inspect_plugin(path, &config) {
                    Ok(report) if report.issues.is_empty() => {
                        // A plugin is any Lua namespace: a resource (has facets) or a helper library
                        // (none) — both valid. Report the shape rather than requiring facets.
                        let detail = match report.shape {
                            Some(prova_core::PluginShape::Resource) => {
                                format!("resource; facets: {}", report.facets.join(", "))
                            }
                            Some(prova_core::PluginShape::Library) => "library".to_string(),
                            None => "namespace".to_string(),
                        };
                        println!("ok   {file}  ({detail})");
                    }
                    Ok(report) => {
                        ok = false;
                        println!("FAIL {file}");
                        for issue in &report.issues {
                            println!("       - {issue}");
                        }
                    }
                    Err(err) => {
                        ok = false;
                        println!("FAIL {file}");
                        println!("       - could not load: {err}");
                    }
                }
            }
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some(other) => {
            eprintln!("prova: unknown plugin subcommand {other:?} (expected: lint)");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: prova plugin lint <file>...");
            ExitCode::from(2)
        }
    }
}

fn run(cli_args: Vec<String>) -> ExitCode {
    let mut cli_format: Option<Format> = None;
    let mut cli_jobs: Option<usize> = None;
    let mut list = false;
    let mut explicit_paths: Vec<String> = Vec::new();
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut cli_plugins: Vec<String> = Vec::new();

    let mut args = cli_args.into_iter();
    while let Some(arg) = args.next() {
        // `--plugin name=source` (repeatable): an ad-hoc plugin, layered over the manifest (CLI wins).
        if let Some(v) = value_flag(&arg, &mut args, &["--plugin", "-P"]) {
            cli_plugins.push(v);
            continue;
        }
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

    // Filesystem layout — where global plugins live (data_dir/plugins) and where git plugins cache.
    let layout = match XdgSystemLayout::new() {
        Ok(layout) => layout,
        Err(err) => {
            eprintln!("prova: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };

    // Resolve the run: explicit path args bypass the manifest; otherwise read prova.toml. Manifest
    // `[plugins]` are resolved (git sources fetched into the cache) into a name→file map.
    let (paths, jobs, format, declared, mut plugins_resolved, sources) = if !explicit_paths.is_empty()
    {
        (
            explicit_paths,
            cli_jobs.unwrap_or(1),
            cli_format.unwrap_or(Format::Console),
            BTreeMap::new(),
            plugins::ResolvedPlugins::default(),
            BTreeMap::new(),
        )
    } else {
        match resolve_from_manifest(manifest_path, profile, cli_jobs, cli_format, &layout) {
            Ok(resolved) => resolved,
            Err(code) => return code,
        }
    };

    // Ad-hoc `--plugin name=source` entries (e.g. CI-only extras) resolve the same way as manifest
    // plugins and layer on top, overriding a manifest plugin of the same name.
    if !cli_plugins.is_empty() {
        let mut adhoc: BTreeMap<String, manifest::PluginSource> = BTreeMap::new();
        for entry in &cli_plugins {
            match entry.split_once('=') {
                Some((name, source)) if !name.is_empty() && !source.is_empty() => {
                    adhoc.insert(name.to_string(), manifest::PluginSource::Path(source.to_string()));
                }
                _ => {
                    eprintln!("prova: --plugin expects name=source, got {entry:?}");
                    return ExitCode::from(2);
                }
            }
        }
        match plugins::resolve_plugins(&adhoc, Path::new("."), &layout, &sources, PROVA_VERSION) {
            Ok(resolved) => {
                plugins_resolved.named.extend(resolved.named);
                plugins_resolved.namespaces.extend(resolved.namespaces);
            }
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::from(2);
            }
        }
    }

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

    // The standalone `prova` binary ships the archetect plugin, so `archetect.render{...}` works.
    // The plugin searcher consults the global install dir plus any manifest-declared plugins.
    let mut config = RunConfig::new(jobs)
        .with_module(prova_archetect::install)
        .with_plugin_root(layout.plugins_dir());
    for (name, path) in &plugins_resolved.named {
        config = config.with_named_plugin(name.clone(), path.clone());
    }
    for (canonical, dir) in &plugins_resolved.namespaces {
        config = config.with_plugin_namespace(canonical.clone(), dir.clone());
    }

    if list {
        for file in suites.iter().flat_map(|s| &s.files) {
            match discover_path_with(file, &config) {
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

    match run_suites(&suites, reporter.as_mut(), &config) {
        Ok(summary) if summary.is_success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}

/// Read `prova.toml` (or `--manifest`), overlay `--profile`, apply env, merge CLI overrides, and
/// resolve declared plugins (fetching git sources into the cache). Returns (paths, jobs, format,
/// declared-suites, named-plugins, source-aliases) or an exit code on error.
#[allow(clippy::type_complexity)]
fn resolve_from_manifest(
    manifest_path: Option<String>,
    profile: Option<String>,
    cli_jobs: Option<usize>,
    cli_format: Option<Format>,
    layout: &dyn SystemLayout,
) -> Result<
    (
        Vec<String>,
        usize,
        Format,
        BTreeMap<String, SuiteDecl>,
        plugins::ResolvedPlugins,
        BTreeMap<String, String>,
    ),
    ExitCode,
> {
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

    // Resolve declared plugins relative to the manifest's directory (git sources fetched into cache).
    let base_dir = Path::new(&path).parent().unwrap_or(Path::new(".")).to_path_buf();
    let plugins_resolved = plugins::resolve_plugins(
        &resolved.plugins,
        &base_dir,
        layout,
        &resolved.sources,
        PROVA_VERSION,
    )
    .map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;

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
    Ok((
        resolved.paths,
        jobs,
        format,
        resolved.suites,
        plugins_resolved,
        resolved.sources,
    ))
}
