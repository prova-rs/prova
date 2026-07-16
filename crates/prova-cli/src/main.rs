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

mod annotations;
mod home;
mod init;
mod manifest;
mod plugins;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use home::Home;
use manifest::{Manage, Manifest, SuiteDecl};
use prova_core::{
    discover_files, discover_path_with, discover_suites, run_suites, ConsoleReporter, JsonReporter,
    MultiReporter, Reporter, RunConfig, Suite, SystemLayout, XdgSystemLayout,
};

const HELP: &str = "\
usage:
  prova <file-or-dir>...    run the given files/dirs
  prova                     run the suite declared in prova.toml (found by walking up)
  prova init                scaffold prova.toml + LuaLS IDE support in this project
  prova up <topology>       stand up a topology and hold it until Ctrl-C
  prova plugin lint <f>...  check plugin files against the namespacing grammar

options:
  -p, --profile NAME        run a profile from the manifest
      --manifest PATH       use a specific manifest (default ./prova.toml)
      --format console|json output format (--json is shorthand)
  -j, --jobs N              run up to N units concurrently
  -P, --plugin name=source  add an ad-hoc plugin (repeatable; layers over the manifest)
  -k PATTERN                select nodes whose path contains PATTERN (repeatable; !PAT excludes)
      --tags a,b            select nodes tagged with any listed tag (repeatable; !tag excludes)
      --node PATH           select an exact node path (repeatable) — re-run what a report named
      --last-failed         select only the nodes that failed in the previous run
      --list                discover tests without running them (respects selection)
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
    // Subcommands: `prova plugin <...>` / `prova init`. Everything else is the run path.
    let mut raw = std::env::args().skip(1).peekable();
    match raw.peek().map(String::as_str) {
        Some("plugin") => {
            raw.next();
            return plugin_subcommand(raw.collect());
        }
        Some("init") => {
            raw.next();
            return init::run(raw.collect());
        }
        Some("up") => {
            raw.next();
            return up_subcommand(raw.collect());
        }
        _ => {}
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
                let ns = plugins::namespace_for_file(path);
                let mut config = RunConfig::new(1).with_module(prova_archetect::install);
                if let Some((canonical, dir)) = &ns {
                    config = config.with_plugin_namespace(canonical.clone(), dir.clone());
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
                        // Advisory (non-fatal): a published plugin should ship a LuaCATS stub so
                        // consumers of `require("<name>")` get editor completion. The archetype
                        // generates it; warn when it's absent so the ecosystem stays IDE-ready.
                        if let Some(warning) = missing_stub_warning(&ns) {
                            println!("     warn: {warning}");
                        }
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

/// `prova up <topology>` — stand up a named topology (the same definition tests use) and hold it
/// running until Ctrl-C, printing each resource's endpoint. Discovers the topology in the manifest's
/// test files, resolves declared plugins, and hands off to the engine's held-execution mode.
fn up_subcommand(args: Vec<String>) -> ExitCode {
    let mut name: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if let Some(v) = value_flag(&arg, &mut it, &["--profile", "-p"]) {
            profile = Some(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut it, &["--manifest"]) {
            manifest_path = Some(v);
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova up <topology> [--profile NAME] [--manifest PATH]\n\
                     \n\
                     stand up a topology (declared with prova.topology) and hold it running until\n\
                     Ctrl-C, printing each resource's endpoint."
                );
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("prova up: unknown flag {other}");
                return ExitCode::from(2);
            }
            other if name.is_none() => name = Some(other.to_string()),
            other => {
                eprintln!("prova up: unexpected argument {other:?} (expected one topology name)");
                return ExitCode::from(2);
            }
        }
    }

    let Some(name) = name else {
        eprintln!("usage: prova up <topology>");
        return ExitCode::from(2);
    };

    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("prova: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };

    // Locate the project (the manifest tells us where topologies + plugins live).
    let home = match &manifest_path {
        Some(p) => home::from_manifest_path(Path::new(p)),
        None => {
            match home::find(Path::new(".")) {
                Ok(Some(h)) => h,
                Ok(None) => {
                    eprintln!("prova up: no prova.toml found (a topology is declared in a project's files)");
                    return ExitCode::from(2);
                }
                Err(e) => {
                    eprintln!("prova: {e}");
                    return ExitCode::from(2);
                }
            }
        }
    };

    let run = match resolve_from_manifest(&home, profile, None, None, &layout) {
        Ok(r) => r,
        Err(code) => return code,
    };

    // Gather every file that could declare a topology: the run paths plus any explicit suites.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut discover = |rel: &str| -> Result<(), ExitCode> {
        match discover_files(&home.dir.join(rel)) {
            Ok(found) => {
                files.extend(found);
                Ok(())
            }
            Err(err) => {
                eprintln!("prova up: {rel}: {err}");
                Err(ExitCode::from(2))
            }
        }
    };
    for p in &run.paths {
        if let Err(code) = discover(p) {
            return code;
        }
    }
    for decl in run.suites.values() {
        for p in &decl.paths {
            if let Err(code) = discover(p) {
                return code;
            }
        }
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        eprintln!("prova up: no files found to search for topologies");
        return ExitCode::from(2);
    }

    // Build the engine config with the declared plugins (so the topology's `require(...)` resolves).
    let mut config = RunConfig::new(1)
        .with_module(prova_archetect::install)
        .with_plugin_root(layout.plugins_dir());
    for (n, path) in &run.plugins.named {
        config = config.with_named_plugin(n.clone(), path.clone());
    }
    for (canonical, dir) in &run.plugins.namespaces {
        config = config.with_plugin_namespace(canonical.clone(), dir.clone());
    }

    eprintln!("prova: standing up topology {name:?}…");
    let result = prova_core::up(&files, &name, &config, |endpoints| {
        println!("\n  {name} — up:");
        if endpoints.is_empty() {
            println!("    (no endpoints — a resource exposes a `url` field to appear here)");
        } else {
            let w = endpoints.iter().map(|e| e.name.len()).max().unwrap_or(0);
            for e in endpoints {
                println!("    {:<w$}  {}", e.name, e.url);
            }
        }
        println!("\n  holding — Ctrl-C to tear down");
    });
    match result {
        Ok(()) => {
            println!("\n  torn down.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("prova up: {err}");
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
    let mut selection = prova_core::Selection::default();
    let mut last_failed = false;

    let mut args = cli_args.into_iter();
    while let Some(arg) = args.next() {
        // `--plugin name=source` (repeatable): an ad-hoc plugin, layered over the manifest (CLI wins).
        if let Some(v) = value_flag(&arg, &mut args, &["--plugin", "-P"]) {
            cli_plugins.push(v);
            continue;
        }
        // `-k pattern` (repeatable): case-insensitive substring of the node path; `!pat` excludes.
        if let Some(v) = value_flag(&arg, &mut args, &["-k"]) {
            match v.strip_prefix('!') {
                Some(rest) => selection.keyword_excludes.push(rest.to_string()),
                None => selection.keywords.push(v),
            }
            continue;
        }
        // `--tags a,b` (repeatable): leaf has any listed tag; `!tag` excludes.
        if let Some(v) = value_flag(&arg, &mut args, &["--tags"]) {
            for t in v.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                match t.strip_prefix('!') {
                    Some(rest) => selection.tag_excludes.push(rest.to_string()),
                    None => selection.tags.push(t.to_string()),
                }
            }
            continue;
        }
        // `--node "full › node › path"` (repeatable): exact node selection — re-run precisely the
        // node a report named.
        if let Some(v) = value_flag(&arg, &mut args, &["--node"]) {
            selection.nodes.push(v);
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
            "--last-failed" => last_failed = true,
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

    // Determine the prova home (the directory owning `prova.toml`), unless explicit path args bypass
    // the manifest. `--manifest PATH` points directly at a manifest; otherwise discovery walks up
    // from the current directory. An ambiguous layout (more than one manifest location) is an error.
    let home: Option<Home> = if !explicit_paths.is_empty() {
        None
    } else if let Some(path) = &manifest_path {
        Some(home::from_manifest_path(Path::new(path)))
    } else {
        match home::find(Path::new(".")) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::from(2);
            }
        }
    };

    // Resolve the run. Explicit path args bypass the manifest (paths relative to cwd, no IDE
    // management); otherwise read the home's `prova.toml` (paths relative to the home dir).
    let (base_dir, paths, jobs, format, declared, mut plugins_resolved, sources, manage) =
        if !explicit_paths.is_empty() {
            (
                PathBuf::from("."),
                explicit_paths,
                cli_jobs.unwrap_or(1),
                cli_format.unwrap_or(Format::Console),
                BTreeMap::new(),
                plugins::ResolvedPlugins::default(),
                BTreeMap::new(),
                Manage::Never,
            )
        } else {
            let Some(home) = &home else {
                eprintln!(
                    "usage: prova <file-or-dir>...   or   prova [--profile NAME]  (reads prova.toml)"
                );
                return ExitCode::from(2);
            };
            match resolve_from_manifest(home, profile, cli_jobs, cli_format, &layout) {
                Ok(r) => (
                    home.dir.clone(),
                    r.paths,
                    r.jobs,
                    r.format,
                    r.suites,
                    r.plugins,
                    r.sources,
                    r.manage,
                ),
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
                    adhoc.insert(
                        name.to_string(),
                        manifest::PluginSource::Path(source.to_string()),
                    );
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
                plugins_resolved.roots.extend(resolved.roots);
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
            match discover_files(&base_dir.join(p)) {
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
                setup: decl.setup.as_ref().map(|s| base_dir.join(s)),
                files,
            });
        }
    }
    for arg in &paths {
        match discover_suites(&base_dir.join(arg)) {
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

    // `--last-failed`: fold the previous run's failed node paths into the selection as exact nodes.
    if last_failed {
        match load_last_failed(&home) {
            Some(paths) if !paths.is_empty() => selection.nodes.extend(paths),
            _ => eprintln!(
                "prova: --last-failed: no failure state from a previous run here; running everything"
            ),
        }
    }
    config.selection = selection;

    // IDE integration: on a manifest run (not read-only `--list`), refresh the annotation folder
    // (core + plugin `---@meta` stubs) and manage `.luarc.json` per `[luals] manage`, so
    // `require("<plugin>")` completes in the editor with no manual wiring. Never blocks the run — a
    // sync error is a warning, not a failure — and all output goes to stderr so `--format json`
    // stdout stays a clean event stream.
    if !list {
        if let Some(home) = &home {
            match annotations::setup(home, &plugins_resolved.roots, manage) {
                Ok(outcome) => report_annotations(&outcome),
                Err(err) => eprintln!("prova: IDE annotations: {err}"),
            }
        }
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

    let inner: Box<dyn Reporter> = match format {
        Format::Console => Box::new(ConsoleReporter),
        Format::Json => Box::new(MultiReporter::new(vec![Box::new(JsonReporter::new(
            std::io::stdout(),
        ))])),
    };
    // Record failed node paths so the next `--last-failed` can re-run exactly them.
    let mut reporter = FailureRecorder {
        inner,
        failed: Vec::new(),
    };

    match run_suites(&suites, &mut reporter, &config) {
        Ok(summary) => {
            store_last_failed(&home, &reporter.failed);
            if summary.is_success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}

/// A resolved manifest run: what to discover, how, and the resolved plugin + IDE settings.
struct ManifestRun {
    paths: Vec<String>,
    jobs: usize,
    format: Format,
    suites: BTreeMap<String, SuiteDecl>,
    plugins: plugins::ResolvedPlugins,
    sources: BTreeMap<String, String>,
    manage: Manage,
}

/// If a linted plugin ships no LuaCATS stub (`library/<canonical>.lua`), return an advisory message.
/// `ns` is `(canonical, plugin_root_dir)` from `namespace_for_file`. Consumers of `require(name)` get
/// no editor completion without a stub — the plugin archetype generates one; this nudges hand-authored
/// plugins to match.
fn missing_stub_warning(ns: &Option<(String, PathBuf)>) -> Option<String> {
    let (canonical, dir) = ns.as_ref()?;
    let stub = dir.join("library").join(format!("{canonical}.lua"));
    if stub.is_file() {
        return None;
    }
    Some(format!(
        "no library/{canonical}.lua — consumers of require(\"{canonical}\") get no editor \
         completion (add a ---@meta stub; the plugin archetype generates one)"
    ))
}

/// Print a concise, honest one-liner (to stderr) about what the IDE annotation sync did.
fn report_annotations(outcome: &annotations::Outcome) {
    if !outcome.synced_plugins.is_empty() {
        eprintln!(
            "prova: synced IDE annotations for {}",
            outcome.synced_plugins.join(", ")
        );
    }
    if outcome.luarc_created {
        eprintln!("prova: wrote .luarc.json (editor IDE support enabled)");
    }
    if outcome.luarc_hint {
        eprintln!("prova: IDE annotations ready — run `prova init` to point .luarc.json at them");
    }
}

/// Read the home's `prova.toml`, overlay `--profile`, apply env, merge CLI overrides, and resolve
/// declared plugins (fetching git sources into the cache). All paths remain manifest-relative (the
/// caller joins them to the home dir). Returns the resolved run or an exit code on error.
fn resolve_from_manifest(
    home: &Home,
    profile: Option<String>,
    cli_jobs: Option<usize>,
    cli_format: Option<Format>,
    layout: &dyn SystemLayout,
) -> Result<ManifestRun, ExitCode> {
    let path = &home.manifest;

    let text = std::fs::read_to_string(path).map_err(|_| {
        eprintln!("prova: cannot read manifest {}", path.display());
        ExitCode::from(2)
    })?;

    let manifest = Manifest::parse(&text).map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;
    let resolved = manifest.resolve(profile.as_deref()).map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;
    if resolved.paths.is_empty() && resolved.suites.is_empty() {
        eprintln!(
            "prova: manifest {} defines no paths or suites to run",
            path.display()
        );
        return Err(ExitCode::from(2));
    }
    let manage = resolved.luals.manage().map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;

    // Apply the run environment before tests execute.
    for (key, value) in &resolved.env {
        std::env::set_var(key, value);
    }

    // Resolve declared plugins relative to the home directory (git sources fetched into cache).
    let plugins_resolved = plugins::resolve_plugins(
        &resolved.plugins,
        &home.dir,
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
    Ok(ManifestRun {
        paths: resolved.paths,
        jobs,
        format,
        suites: resolved.suites,
        plugins: plugins_resolved,
        sources: resolved.sources,
        manage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_stub_warning_fires_only_without_a_stub() {
        let base = std::env::temp_dir().join(format!("prova-lint-stub-{}", std::process::id()));
        let dir = base.join("plugin");
        std::fs::create_dir_all(&dir).unwrap();
        let ns = Some(("postgres".to_string(), dir.clone()));

        // No library/ → warns.
        let w = missing_stub_warning(&ns).expect("should warn without a stub");
        assert!(w.contains("library/postgres.lua"), "{w}");

        // With library/postgres.lua → silent.
        std::fs::create_dir_all(dir.join("library")).unwrap();
        std::fs::write(
            dir.join("library").join("postgres.lua"),
            "---@meta postgres\n",
        )
        .unwrap();
        assert!(missing_stub_warning(&ns).is_none());

        // No namespace at all (headless file with no parent info) → nothing to advise.
        assert!(missing_stub_warning(&None).is_none());

        std::fs::remove_dir_all(&base).ok();
    }
}

/// Forwards every event and records the paths of failed nodes, so `--last-failed` can select
/// exactly them next run.
struct FailureRecorder {
    inner: Box<dyn Reporter>,
    failed: Vec<String>,
}

impl Reporter for FailureRecorder {
    fn event(&mut self, event: &prova_core::Event) {
        if let prova_core::Event::NodeFinished {
            path,
            outcome: prova_core::Outcome::Failed,
            ..
        } = event
        {
            self.failed.push(path.to_string());
        }
        self.inner.event(event);
    }
}

/// Where `--last-failed` state lives: a small JSON list of node paths in the prova home. Runs
/// without a manifest home have nowhere durable to record, so the feature quietly no-ops there.
fn last_failed_file(home: &Option<home::Home>) -> Option<std::path::PathBuf> {
    home.as_ref().map(|h| h.dir.join(".last-failed.json"))
}

fn load_last_failed(home: &Option<home::Home>) -> Option<Vec<String>> {
    let path = last_failed_file(home)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn store_last_failed(home: &Option<home::Home>, failed: &[String]) {
    let Some(path) = last_failed_file(home) else {
        return;
    };
    if failed.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(failed) {
        let _ = std::fs::write(path, text);
    }
}
