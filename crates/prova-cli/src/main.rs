//! `prova` CLI.
//!
//! Usage:
//!   prova <file-or-dir>...           run the given files/dirs (console output)
//!   prova                            run the suite declared in ./prova.toml
//!   prova --profile ci               run the `ci` profile from ./prova.toml
//!   prova --manifest path.toml       use a specific manifest
//!   prova --format json <path>       stream JSONL events (machine/GUI protocol)
//!   prova --format tap <path>        emit TAP (Test Anything Protocol) to stdout
//!   prova --junit results.xml <path> also write a JUnit XML report (for CI dashboards)
//!   prova --list <path>              discover tests without running them
//!   prova --jobs N <path>            run up to N units concurrently (throughput only)
//!
//! CLI flags override manifest values; explicit path arguments bypass the manifest entirely.

mod annotations;
mod catalog;
mod home;
mod ide;
mod init;
mod manifest;
mod mcp;
mod plugins;
mod runstate;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use home::Home;
use manifest::{Manage, Manifest, SuiteDecl};
use prova_core::{
    discover_files, discover_path_with, discover_suites, run_suites, ConsoleReporter,
    JUnitReporter, JsonReporter, MultiReporter, PortMode, Reporter, RunConfig, Suite, SystemLayout,
    TapReporter, XdgSystemLayout,
};

const HELP: &str = "\
usage:
  prova <file-or-dir>...    run the given files/dirs
  prova                     run the suite declared in prova.toml (found by walking up)
  prova init [<key>]        render a catalog archetype into this project (interactive if no key),
                            then wire LuaLS IDE support
  prova init --list         list the init catalog: the archetypes prova can scaffold from
  prova ide setup           (re)wire this project's LuaLS support: core stubs + .luarc.json
  prova eval '<code>'       run a one-shot Lua snippet in the full prova environment and print
                            the returned value (`-` reads the snippet from stdin)
  prova skill               print the agent skill (how to drive Prova); --install writes it
                            to .claude/skills/prova/SKILL.md at the project root
  prova mcp                 serve an MCP stdio server whose tools mirror the CLI (run, list, eval)
  prova up [<topology>]     list defined topologies, or stand one up and hold it until Ctrl-C (--fixed)
  prova watch <topology>    stand up a topology and re-apply on definition change (dev loop)
  prova start <topology>    stand up a topology detached (returns; use `down` to stop)
  prova down <topology>     tear down a detached topology
  prova ps                  list running topologies
  prova plugin lint <f>...  check plugin files against the namespacing grammar

options:
  -p, --profile NAME        run a profile from the manifest
      --manifest PATH       use a specific manifest (default ./prova.toml)
      --format console|json|tap  output format (--json is shorthand)
      --junit PATH          also write a JUnit XML report to PATH (for CI; composes with --format)
  -j, --jobs N              run up to N units concurrently
  -P, --plugin name=source  add an ad-hoc plugin (repeatable; layers over the manifest)
  -k PATTERN                select nodes whose path contains PATTERN (repeatable; !PAT excludes)
      --tags a,b            select nodes tagged with any listed tag (repeatable; !tag excludes)
      --node PATH           select an exact node path (repeatable) — re-run what a report named
      --last-failed         select only the nodes that failed in the previous run
      --allow-empty         a selection matching no tests is OK (default: that is an error)
  -u, --update-snapshots    (re)write snapshots instead of comparing (matches_snapshot)
      --unreferenced M      snapshots no test used: ignore (default) | warn | delete (full runs only)
  -U, --update              force-refresh git plugin sources (skip the freshness cache)
      --offline             never fetch git plugin sources; use only what is already cached
      --list                discover tests without running them (respects selection)
  -V, --version             print version
  -h, --help                print this help";

/// The running prova version, checked against each plugin's `requires.prova` compatibility range.
const PROVA_VERSION: &str = env!("CARGO_PKG_VERSION");

enum Format {
    Console,
    Json,
    Tap,
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
        Some("skill") => {
            raw.next();
            return skill_subcommand(raw.collect());
        }
        Some("init") => {
            raw.next();
            return init::run(raw.collect());
        }
        Some("ide") => {
            raw.next();
            return ide::run(raw.collect());
        }
        Some("eval") => {
            raw.next();
            return eval_subcommand(raw.collect());
        }
        Some("mcp") => {
            raw.next();
            return mcp::run(raw.collect());
        }
        Some("up") => {
            raw.next();
            return up_subcommand(raw.collect());
        }
        Some("watch") => {
            raw.next();
            return watch_subcommand(raw.collect());
        }
        Some("start") => {
            raw.next();
            return start_subcommand(raw.collect());
        }
        Some("down") => {
            raw.next();
            return down_subcommand(raw.collect());
        }
        Some("ps") => {
            raw.next();
            return ps_subcommand(raw.collect());
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

/// `prova eval '<code>'` — run a one-shot Lua snippet in the FULL prova environment (built-in
/// modules, manifest-declared plugins via `require`, a real transient `ctx`) and print the returned
/// value. Goes through the same manifest/home/plugins resolution as the run path, so
/// `require("postgres")` works from a project directory; without a manifest it still runs with the
/// built-ins. Exit 0 on success, 1 if the snippet raises, 2 on usage errors.
fn eval_subcommand(args: Vec<String>) -> ExitCode {
    let mut code: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut cli_plugins: Vec<String> = Vec::new();
    let mut force_json = false;

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
        if let Some(v) = value_flag(&arg, &mut it, &["--plugin", "-P"]) {
            cli_plugins.push(v);
            continue;
        }
        if let Some(v) = value_flag(&arg, &mut it, &["--format"]) {
            match v.as_str() {
                "json" => force_json = true,
                "console" => {}
                other => {
                    eprintln!("prova eval: unknown format {other:?} (expected console|json)");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        match arg.as_str() {
            "--json" => force_json = true,
            "-h" | "--help" => {
                println!(
                    "usage: prova eval '<lua code>' [--format json] [--profile NAME] [--manifest PATH] [-P name=source]\n\
                     \n\
                     run a one-shot Lua snippet in the full prova environment — built-in modules\n\
                     (fs, shell, docker, http, …), manifest-declared plugins via require(), and a\n\
                     real transient `ctx` (anything it provisions is torn down afterwards) — then\n\
                     print the returned value and exit.\n\
                     \n\
                     the snippet may be a bare expression (`1 + 1`) or statements with an explicit\n\
                     `return`. pass `-` to read the snippet from stdin.\n\
                     \n\
                     examples:\n\
                     \x20 prova eval 'return 1 + 1'\n\
                     \x20 prova eval 'return fs.exists(\"Cargo.toml\")'\n\
                     \x20 prova eval 'local db = require(\"postgres\").container(ctx); return db.url'"
                );
                return ExitCode::SUCCESS;
            }
            "-" if code.is_none() => {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("prova eval: cannot read snippet from stdin: {e}");
                    return ExitCode::from(2);
                }
                code = Some(buf);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                eprintln!("prova eval: unknown flag {other}");
                return ExitCode::from(2);
            }
            other if code.is_none() => code = Some(other.to_string()),
            other => {
                eprintln!("prova eval: unexpected argument {other:?} (expected one snippet)");
                return ExitCode::from(2);
            }
        }
    }

    let Some(code) = code else {
        eprintln!(
            "usage: prova eval '<lua code>'   (or `prova eval -` to read the snippet from stdin)"
        );
        return ExitCode::from(2);
    };
    if code.trim().is_empty() {
        eprintln!("prova eval: the snippet is empty");
        return ExitCode::from(2);
    }

    let layout = match XdgSystemLayout::new() {
        Ok(layout) => layout,
        Err(err) => {
            eprintln!("prova: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };

    // Same home/manifest resolution as the run path — but a missing manifest is fine here: the
    // snippet then runs with just the built-ins (no manifest-declared plugins).
    let home: Option<Home> = if let Some(path) = &manifest_path {
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
    let (mut plugins_resolved, sources) = match &home {
        Some(home) => {
            match resolve_from_manifest(home, profile, None, None, None, &layout, false, false) {
                Ok(r) => (r.plugins, r.sources),
                Err(code) => return code,
            }
        }
        None => (plugins::ResolvedPlugins::default(), BTreeMap::new()),
    };
    if let Err(code) = layer_cli_plugins(&cli_plugins, &layout, &sources, &mut plugins_resolved) {
        return code;
    }
    let config = engine_config(1, &plugins_resolved, home.as_ref());

    match prova_core::eval_snippet(&code, &config) {
        Ok(value) => {
            print_eval_value(&value, force_json);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("prova eval: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Print an eval result: scalars plainly (a string without quotes, so the value is shell-friendly),
/// nothing for null, pretty JSON for tables/arrays. `--format json` forces JSON for everything.
fn print_eval_value(value: &serde_json::Value, force_json: bool) {
    use serde_json::Value as J;
    if force_json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
        );
        return;
    }
    match value {
        J::Null => {}
        J::Bool(b) => println!("{b}"),
        J::Number(n) => println!("{n}"),
        J::String(s) => println!("{s}"),
        other => println!(
            "{}",
            serde_json::to_string_pretty(other).unwrap_or_else(|_| "null".into())
        ),
    }
}

/// `prova up <topology>` — stand up a named topology (the same definition tests use) and hold it
/// running until Ctrl-C, printing each resource's endpoint. Discovers the topology in the manifest's
/// test files, resolves declared plugins, and hands off to the engine's held-execution mode.
fn up_subcommand(args: Vec<String>) -> ExitCode {
    let mut name: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut fixed = false;

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
            "--fixed" => {
                fixed = true;
                continue;
            }
            "-h" | "--help" => {
                println!(
                    "usage: prova up [<topology>] [--fixed] [--profile NAME] [--manifest PATH]\n\
                     \n\
                     with no topology, list the topologies this project defines.\n\
                     with one, stand it up (declared with prova.topology) and hold it running until\n\
                     Ctrl-C, printing each resource's endpoint.\n\
                     \n\
                     --fixed  pin each resource to its canonical container port on the host (a\n\
                     \x20        predictable, external-tool-friendly address) instead of a random one.\n\
                     \x20        Only one fixed instance of a port can run at a time."
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

    // No name → the discovery form: list what's defined (like `prova init` listing templates).
    let Some(name) = name else {
        return up_list(profile, manifest_path);
    };

    let prep = match build_topology_run("up", Some(&name), profile, manifest_path, fixed) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let TopologyRun {
        home,
        files,
        config,
        ..
    } = prep;

    // Refuse to double-provision: if a live record for this name exists, it is already up. A stale
    // record (the holder is gone) is cleared and we proceed.
    if let Some(rec) = runstate::read(&home, &name) {
        if runstate::is_alive(rec.pid) {
            eprintln!(
                "prova up: topology {name:?} is already up (pid {})",
                rec.pid
            );
            return ExitCode::from(2);
        }
        runstate::remove(&home, &name);
    }

    eprintln!("prova: standing up topology {name:?}…");
    // Self-register run-state once provisioned, so `prova down`/`ps` can supervise this holder (the
    // same for an attached `up` here and the detached child a `prova start` spawns).
    let state_home = home.clone();
    let state_name = name.clone();
    let result = prova_core::up(&files, &name, &config, |endpoints| {
        let record = runstate::Record {
            name: state_name.clone(),
            pid: std::process::id(),
            started_at: runstate::now_secs(),
            endpoints: endpoints
                .iter()
                .map(|e| runstate::Endpoint {
                    name: e.name.clone(),
                    url: e.url.clone(),
                })
                .collect(),
        };
        if let Err(e) = runstate::write(&state_home, &record) {
            eprintln!("prova up: could not record run-state: {e}");
        }
        print_endpoints(&state_name, endpoints);
        println!("\n  holding — Ctrl-C to tear down");
    });
    // Clean teardown completed (or provisioning failed) — drop our record.
    runstate::remove(&home, &name);
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

/// `prova up` with no name — list the topologies this project defines, so you can see what's there
/// before standing one up (the mirror of `prova init` listing templates). Only registers topologies
/// (execs the definition files); no factory runs, so this needs no docker.
fn up_list(profile: Option<String>, manifest_path: Option<String>) -> ExitCode {
    let prep = match build_topology_run("up", None, profile, manifest_path, false) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let names = match prova_core::list_topologies(&prep.files, &prep.config) {
        Ok(names) => names,
        Err(err) => {
            eprintln!("prova up: {err}");
            return ExitCode::from(2);
        }
    };
    if names.is_empty() {
        eprintln!(
            "prova up: no topologies defined — declare one with prova.topology(name, fn) in a suite"
        );
        return ExitCode::from(2);
    }
    println!("topologies ({}):", names.len());
    for name in &names {
        println!("  {name}");
    }
    println!("\nstand one up with `prova up <topology>`.");
    ExitCode::SUCCESS
}

/// Everything the `up`/`watch` verbs need to stand a topology up: the located project, the files that
/// may declare topologies, and the engine config (plugins resolved, port mode set).
struct TopologyRun {
    home: Home,
    files: Vec<PathBuf>,
    config: RunConfig,
}

/// Resolve the manifest, discover the topology files, and build the engine config for an inhabited
/// verb (`up`/`watch`). Shared so both consume one definition the same way; `verb` only labels errors.
fn build_topology_run(
    verb: &str,
    name: Option<&str>,
    profile: Option<String>,
    manifest_path: Option<String>,
    fixed: bool,
) -> Result<TopologyRun, ExitCode> {
    let layout = XdgSystemLayout::new().map_err(|err| {
        eprintln!("prova: cannot determine home directories: {err}");
        ExitCode::from(2)
    })?;

    // Locate the project (the manifest tells us where topologies + plugins live).
    let home = resolve_home(manifest_path.as_deref())?;

    let run = resolve_from_manifest(&home, profile, None, None, None, &layout, false, false)?;

    // Gather every file that could declare a topology: the run paths plus any explicit suites.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut discover = |rel: &str| -> Result<(), ExitCode> {
        match discover_files(&home.dir.join(rel)) {
            Ok(found) => {
                files.extend(found);
                Ok(())
            }
            Err(err) => {
                eprintln!("prova {verb}: {rel}: {err}");
                Err(ExitCode::from(2))
            }
        }
    };
    for p in &run.paths {
        discover(p)?;
    }
    for decl in run.suites.values() {
        for p in &decl.paths {
            discover(p)?;
        }
    }
    files.sort();
    files.dedup();
    // A topology can come from a file (`prova.topology`) OR the manifest (`[topologies]`), so only
    // error on "nothing to load" when BOTH are empty.
    if files.is_empty() && run.topologies.is_empty() {
        match name {
            Some(n) => {
                eprintln!("prova {verb}: no files found to search for topologies (topology {n:?})")
            }
            None => eprintln!("prova {verb}: no topologies defined (nothing under the run paths)"),
        }
        return Err(ExitCode::from(2));
    }

    // Build the engine config with the declared plugins (so the topology's `require(...)` resolves).
    // `--fixed` pins ports for external reachability; the default is random (like tests), so several
    // topologies can be inhabited at once without colliding.
    let mut config = engine_config(1, &run.plugins, Some(&home)).with_ports(if fixed {
        PortMode::Fixed
    } else {
        PortMode::Auto
    });
    // Manifest topologies (`[topologies]`) desugar to `prova.topology` registrations the engine execs
    // after the files — so `prova up <name>` and the listing form see them as first-class.
    for (alias, decl) in &run.topologies {
        config = config.with_topology_registration(alias, &decl.plugin, &decl.factory);
    }

    Ok(TopologyRun {
        home,
        files,
        config,
    })
}

/// `prova watch <topology>` — the inhabited dev loop: stand the topology up, print its endpoints, and
/// re-provision whenever its definition files change, holding until Ctrl-C. Attached-only (no detached
/// supervisor); pair with `--fixed` for endpoints that stay put across re-applies.
fn watch_subcommand(args: Vec<String>) -> ExitCode {
    let mut name: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut fixed = false;

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
            "--fixed" => {
                fixed = true;
                continue;
            }
            "-h" | "--help" => {
                println!(
                    "usage: prova watch <topology> [--fixed] [--profile NAME] [--manifest PATH]\n\
                     \n\
                     stand up a topology and re-provision it whenever its definition files change,\n\
                     holding until Ctrl-C. A live dev loop over the same definition your tests use.\n\
                     \n\
                     --fixed  keep endpoints on canonical ports so they stay stable across re-applies."
                );
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("prova watch: unknown flag {other}");
                return ExitCode::from(2);
            }
            other if name.is_none() => name = Some(other.to_string()),
            other => {
                eprintln!(
                    "prova watch: unexpected argument {other:?} (expected one topology name)"
                );
                return ExitCode::from(2);
            }
        }
    }

    let Some(name) = name else {
        eprintln!("usage: prova watch <topology>");
        return ExitCode::from(2);
    };

    let TopologyRun { files, config, .. } =
        match build_topology_run("watch", Some(&name), profile, manifest_path, fixed) {
            Ok(p) => p,
            Err(code) => return code,
        };

    eprintln!("prova: watching topology {name:?} (Ctrl-C to stop)…");
    let result = prova_core::watch(
        &files,
        &name,
        &config,
        |endpoints, reapply| {
            if reapply {
                println!("\n  change detected — re-applied:");
            }
            print_endpoints(&name, endpoints);
            println!("\n  watching — edit the definition to re-apply, Ctrl-C to tear down");
        },
        |err| {
            eprintln!(
                "\n  prova watch: provisioning failed — fix the definition to retry:\n    {err}"
            );
        },
    );
    match result {
        Ok(()) => {
            println!("\n  torn down.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("prova watch: {err}");
            ExitCode::from(2)
        }
    }
}

/// Locate the prova project home from `--manifest` or by walking up from the current directory.
fn resolve_home(manifest_path: Option<&str>) -> Result<Home, ExitCode> {
    match manifest_path {
        Some(p) => Ok(home::from_manifest_path(Path::new(p))),
        None => match home::find(Path::new(".")) {
            Ok(Some(h)) => Ok(h),
            Ok(None) => {
                eprintln!("prova: no prova.toml found in this directory or any parent");
                Err(ExitCode::from(2))
            }
            Err(e) => {
                eprintln!("prova: {e}");
                Err(ExitCode::from(2))
            }
        },
    }
}

/// Print a topology's endpoints as an aligned `name → url` block.
fn print_endpoints(name: &str, endpoints: &[prova_core::Endpoint]) {
    println!("\n  {name} — up:");
    if endpoints.is_empty() {
        println!("    (no endpoints — a resource exposes a `url` field to appear here)");
    } else {
        let w = endpoints.iter().map(|e| e.name.len()).max().unwrap_or(0);
        for e in endpoints {
            println!("    {:<w$}  {}", e.name, e.url);
        }
    }
}

/// `prova start <topology>` — stand up a topology **detached**: spawn `prova up <topology>` in its own
/// process group (stdio → a log file), wait for it to self-register (confirming it's up), print the
/// endpoints, and return, leaving it running. `prova down` stops it.
fn start_subcommand(args: Vec<String>) -> ExitCode {
    let (name, manifest_path, profile, fixed) = match parse_topology_args("start", args) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let home = match resolve_home(manifest_path.as_deref()) {
        Ok(h) => h,
        Err(code) => return code,
    };

    if let Some(rec) = runstate::read(&home, &name) {
        if runstate::is_alive(rec.pid) {
            eprintln!(
                "prova start: topology {name:?} is already up (pid {})",
                rec.pid
            );
            return ExitCode::from(2);
        }
        runstate::remove(&home, &name);
    }
    if let Err(e) = runstate::dir(&home) {
        eprintln!("prova start: cannot create run-state dir: {e}");
        return ExitCode::from(2);
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("prova start: cannot find the prova executable: {e}");
            return ExitCode::from(2);
        }
    };
    let log = runstate::log_path(&home, &name);
    let mut cmd = Command::new(exe);
    cmd.arg("up").arg(&name);
    if fixed {
        cmd.arg("--fixed");
    }
    if let Some(m) = &manifest_path {
        cmd.arg("--manifest").arg(m);
    }
    if let Some(p) = &profile {
        cmd.arg("--profile").arg(p);
    }
    if let Err(e) = runstate::detach(&mut cmd, &log) {
        eprintln!("prova start: cannot open log {}: {e}", log.display());
        return ExitCode::from(2);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("prova start: cannot spawn `prova up`: {e}");
            return ExitCode::from(2);
        }
    };

    eprintln!("prova: starting topology {name:?} (detached)…");
    // Poll until the child self-registers (up) or exits (failed). Provisioning can be slow (image
    // pulls, first-boot restarts), so allow a generous window.
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let Some(rec) = runstate::read(&home, &name) {
            let eps: Vec<prova_core::Endpoint> = rec
                .endpoints
                .iter()
                .map(|e| prova_core::Endpoint {
                    name: e.name.clone(),
                    url: e.url.clone(),
                })
                .collect();
            print_endpoints(&name, &eps);
            println!(
                "\n  started (pid {}) — `prova down {name}` to stop, `prova ps` to list",
                rec.pid
            );
            return ExitCode::SUCCESS;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                eprintln!(
                    "prova start: topology {name:?} failed to come up (child exited: {status})"
                );
                let tail = runstate::log_tail(&home, &name, 20);
                if !tail.trim().is_empty() {
                    eprintln!("--- {name} log (tail) ---\n{tail}");
                }
                runstate::remove(&home, &name);
                return ExitCode::from(2);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("prova start: lost track of the child process: {e}");
                return ExitCode::from(2);
            }
        }
        if Instant::now() >= deadline {
            eprintln!("prova start: topology {name:?} did not come up within 300s; stopping it");
            let _ = child.kill();
            runstate::remove(&home, &name);
            return ExitCode::from(2);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `prova down <topology>` — tear down a detached topology by signalling its holder (SIGTERM), which
/// runs the same in-process teardown an attached Ctrl-C would. Idempotent: a missing or stale record
/// is not an error.
fn down_subcommand(args: Vec<String>) -> ExitCode {
    let (name, manifest_path, _profile, _fixed) = match parse_topology_args("down", args) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let home = match resolve_home(manifest_path.as_deref()) {
        Ok(h) => h,
        Err(code) => return code,
    };

    let Some(rec) = runstate::read(&home, &name) else {
        println!("topology {name:?} is not running");
        return ExitCode::SUCCESS;
    };

    if !runstate::is_alive(rec.pid) {
        runstate::remove(&home, &name);
        println!("topology {name:?} was not running (stale record cleaned)");
        return ExitCode::SUCCESS;
    }

    eprintln!("prova: tearing down topology {name:?} (pid {})…", rec.pid);
    runstate::terminate(rec.pid);
    // The holder runs its teardown, then removes its own record and exits — wait for it to be gone.
    let deadline = Instant::now() + Duration::from_secs(120);
    while runstate::is_alive(rec.pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    if runstate::is_alive(rec.pid) {
        eprintln!(
            "prova down: {name:?} (pid {}) did not exit within 120s",
            rec.pid
        );
        runstate::remove(&home, &name);
        return ExitCode::from(2);
    }
    runstate::remove(&home, &name);
    println!("torn down {name}.");
    ExitCode::SUCCESS
}

/// `prova ps` — list this project's running topologies and their endpoints. Stale records (holder
/// gone) are reported once and cleaned up.
fn ps_subcommand(args: Vec<String>) -> ExitCode {
    // `ps` takes only an optional --manifest.
    let mut manifest_path: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if let Some(v) = value_flag(&arg, &mut it, &["--manifest"]) {
            manifest_path = Some(v);
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                println!("usage: prova ps [--manifest PATH]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("prova ps: unexpected argument {other:?}");
                return ExitCode::from(2);
            }
        }
    }
    let home = match resolve_home(manifest_path.as_deref()) {
        Ok(h) => h,
        Err(code) => return code,
    };

    let records = runstate::list(&home);
    if records.is_empty() {
        println!("no topologies running");
        return ExitCode::SUCCESS;
    }
    let now = runstate::now_secs();
    for rec in &records {
        let alive = runstate::is_alive(rec.pid);
        if !alive {
            runstate::remove(&home, &rec.name);
        }
        let status = if alive { "running" } else { "stale" };
        let uptime = now.saturating_sub(rec.started_at);
        println!(
            "{}  [{}]  pid {}  up {}s",
            rec.name, status, rec.pid, uptime
        );
        for e in &rec.endpoints {
            println!("    {}  {}", e.name, e.url);
        }
    }
    ExitCode::SUCCESS
}

/// Parse `<topology> [--fixed] [--profile NAME] [--manifest PATH]` for the `start`/`down` verbs.
/// The `fixed` flag is meaningful only for `start` (forwarded to the detached `prova up`); `down`
/// accepts and ignores it so the two verbs share one parser.
fn parse_topology_args(
    verb: &str,
    args: Vec<String>,
) -> Result<(String, Option<String>, Option<String>, bool), ExitCode> {
    let mut name: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut fixed = false;
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
            "--fixed" => {
                fixed = true;
                continue;
            }
            "-h" | "--help" => {
                println!(
                    "usage: prova {verb} <topology> [--fixed] [--profile NAME] [--manifest PATH]"
                );
                return Err(ExitCode::SUCCESS);
            }
            other if other.starts_with('-') => {
                eprintln!("prova {verb}: unknown flag {other}");
                return Err(ExitCode::from(2));
            }
            other if name.is_none() => name = Some(other.to_string()),
            other => {
                eprintln!(
                    "prova {verb}: unexpected argument {other:?} (expected one topology name)"
                );
                return Err(ExitCode::from(2));
            }
        }
    }
    match name {
        Some(n) => Ok((n, manifest_path, profile, fixed)),
        None => {
            eprintln!("usage: prova {verb} <topology>");
            Err(ExitCode::from(2))
        }
    }
}

fn run(cli_args: Vec<String>) -> ExitCode {
    let mut cli_format: Option<Format> = None;
    let mut cli_junit: Option<String> = None;
    let mut cli_jobs: Option<usize> = None;
    let mut update_snapshots = false;
    let mut unreferenced = String::from("ignore"); // ignore | warn | delete
    let mut cli_config: Option<String> = None;
    let mut list = false;
    let mut explicit_paths: Vec<String> = Vec::new();
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut cli_plugins: Vec<String> = Vec::new();
    let mut selection = prova_core::Selection::default();
    let mut last_failed = false;
    // `--allow-empty`: opt out of the empty-selection error, for the matrix leg that legitimately
    // selects nothing. Off by default, because a selection matching nothing is nearly always a typo
    // and a typo must not be green.
    let mut allow_empty = false;
    // Git-source freshness overrides for this run. `-U`/`--update` forces plugin updates (skips the
    // TTL + remote-hash gates); `--offline` forbids any network, using only what's already cached.
    let mut update_force = false;
    let mut offline = false;

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
                "tap" => cli_format = Some(Format::Tap),
                other => {
                    eprintln!("prova: unknown format {other:?} (expected console|json|tap)");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        // `--junit PATH`: write a JUnit XML report to a file, alongside whatever --format prints.
        if let Some(v) = value_flag(&arg, &mut args, &["--junit"]) {
            cli_junit = Some(v);
            continue;
        }
        // `--unreferenced ignore|warn|delete`: what to do with `.snap` files no test referenced.
        if let Some(v) = value_flag(&arg, &mut args, &["--unreferenced"]) {
            match v.as_str() {
                "ignore" | "warn" | "delete" => unreferenced = v,
                other => {
                    eprintln!(
                        "prova: unknown --unreferenced {other:?} (expected ignore|warn|delete)"
                    );
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        // `--config PATH`: override the companion config file (else `PROVA_CONFIG`, else the manifest
        // `config`, else `prova.lua`). Chiefly a testing affordance — point a run at a specific
        // config without editing a manifest.
        if let Some(v) = value_flag(&arg, &mut args, &["--config"]) {
            cli_config = Some(v);
            continue;
        }
        match arg.as_str() {
            "--list" => list = true,
            "--last-failed" => last_failed = true,
            "--allow-empty" => allow_empty = true,
            "--update-snapshots" | "-u" => update_snapshots = true,
            "--update" | "-U" => update_force = true,
            "--offline" => offline = true,
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
    let (
        base_dir,
        paths,
        jobs,
        format,
        declared,
        mut plugins_resolved,
        sources,
        manage,
        capabilities,
    ) = if !explicit_paths.is_empty() {
        (
            PathBuf::from("."),
            explicit_paths,
            cli_jobs.unwrap_or(1),
            cli_format.unwrap_or(Format::Console),
            BTreeMap::new(),
            plugins::ResolvedPlugins::default(),
            BTreeMap::new(),
            Manage::Never,
            // Explicit-path runs bypass the manifest, so there is no companion — built-in
            // capabilities still work; registered ones are simply absent.
            prova_core::Capabilities::default(),
        )
    } else {
        let Some(home) = &home else {
            eprintln!(
                "usage: prova <file-or-dir>...   or   prova [--profile NAME]  (reads prova.toml)"
            );
            return ExitCode::from(2);
        };
        match resolve_from_manifest(
            home,
            profile,
            cli_jobs,
            cli_format,
            cli_config,
            &layout,
            update_force,
            offline,
        ) {
            Ok(r) => (
                // Test `paths` resolve against the project ROOT, not the home dir — so `proofs/`
                // lives at the root while `.prova/` (or `prova/`) is prova's config/plugins nook.
                // (For a flat `prova.toml`, root == home, so nothing changes.) The `config`
                // companion stays home-relative; only test discovery is root-anchored.
                home.dir.clone(),
                r.paths,
                r.jobs,
                r.format,
                r.suites,
                r.plugins,
                r.sources,
                r.manage,
                r.capabilities,
            ),
            Err(code) => return code,
        }
    };

    // Ad-hoc `--plugin name=source` entries (e.g. CI-only extras) resolve the same way as manifest
    // plugins and layer on top, overriding a manifest plugin of the same name.
    if let Err(code) = layer_cli_plugins(&cli_plugins, &layout, &sources, &mut plugins_resolved) {
        return code;
    }

    // Build the suites to run (declared `[suites.*]` first, then the plain paths).
    let suites = match collect_suites(&base_dir, &declared, &paths) {
        Ok(suites) => suites,
        Err(msg) => {
            eprintln!("prova: {msg}");
            return ExitCode::from(2);
        }
    };
    if suites.is_empty() {
        eprintln!("prova: no test files found (looked for *_test.lua / *.test.lua)");
        return ExitCode::from(2);
    }

    // The standalone `prova` binary ships the archetect plugin, so `archetect.render{...}` works.
    // The plugin searcher consults the global install dir plus any manifest-declared plugins.
    let mut config = engine_config(jobs, &plugins_resolved, home.as_ref())
        .with_update_snapshots(update_snapshots)
        .with_capabilities(capabilities);

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

    // `--unreferenced warn|delete`: track referenced `.snap` files so we can reconcile orphans after
    // the run. Sound only on a **full** run — a selection (`-k`/`--tags`/`--node`/`--last-failed`)
    // would make unrun tests' snapshots look orphaned — so skip (with a note) when a filter is active.
    let snapshot_registry = if unreferenced != "ignore" {
        if config.selection.is_empty() {
            let reg: prova_core::SnapshotRegistry =
                std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
            config = config.with_snapshot_tracking(reg.clone());
            Some(reg)
        } else {
            eprintln!(
                "prova: --unreferenced is skipped on a filtered run (it needs the full suite to be sound)"
            );
            None
        }
    } else {
        None
    };

    // IDE integration: on a manifest run (not read-only `--list`), refresh the annotation folder
    // (core + plugin `---@meta` stubs) and manage `.luarc.json` per `[luals] manage`, so
    // `require("<plugin>")` completes in the editor with no manual wiring. Never blocks the run — a
    // sync error is a warning, not a failure — and all output goes to stderr so `--format json`
    // stdout stays a clean event stream.
    if !list {
        if let Some(home) = &home {
            match annotations::setup(
                home,
                &plugins_resolved.roots,
                manage,
                &layout,
                PROVA_VERSION,
            ) {
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

    // The stdout sink chosen by --format, plus an optional JUnit XML *file* sink (--junit), fanned
    // out through a MultiReporter so a CI run can print to the console and drop a results.xml at once.
    let mut sinks: Vec<Box<dyn Reporter>> = vec![match format {
        Format::Console => Box::new(ConsoleReporter),
        Format::Json => Box::new(JsonReporter::new(std::io::stdout())),
        Format::Tap => Box::new(TapReporter::new(std::io::stdout())),
    }];
    if let Some(path) = &cli_junit {
        match std::fs::File::create(path) {
            Ok(file) => sinks.push(Box::new(JUnitReporter::new(file, "prova"))),
            Err(e) => {
                eprintln!("prova: cannot open --junit file {path:?}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    // Record failed node paths so the next `--last-failed` can re-run exactly them.
    let mut reporter = FailureRecorder {
        inner: Box::new(MultiReporter::new(sinks)),
        failed: Vec::new(),
    };

    match run_suites(&suites, &mut reporter, &config) {
        Ok(summary) => {
            store_last_failed(&home, &reporter.failed);

            // An explicit selection that matched NOTHING is an error, not a green run.
            //
            // The selection axis's instance of the contract: `-k` is *intent*, and a run that asked
            // for something and got nothing did not succeed at it — it usually means a typo, and a
            // typo must not be green. (Distinct from `requires`, which is *ability*: that skips, and
            // is a declared hole rather than a mistake.) Exit 2, with the other usage errors: nothing
            // failed a test.
            let ran = summary.passed + summary.failed + summary.skipped;
            if ran == 0 && !config.selection.is_empty() && !allow_empty {
                let mut asked: Vec<String> = Vec::new();
                asked.extend(
                    config
                        .selection
                        .keywords
                        .iter()
                        .map(|k| format!("-k {k:?}")),
                );
                asked.extend(
                    config
                        .selection
                        .tags
                        .iter()
                        .map(|t| format!("--tags {t:?}")),
                );
                asked.extend(
                    config
                        .selection
                        .nodes
                        .iter()
                        .map(|n| format!("--node {n:?}")),
                );
                eprintln!(
                    "prova: selection matched no tests ({}) — {} deselected",
                    asked.join(", "),
                    summary.deselected
                );
                eprintln!(
                    "prova: a selection that matches nothing is usually a typo; pass --allow-empty if \
                     selecting nothing is intended here."
                );
                return ExitCode::from(2);
            }

            // Reconcile unreferenced snapshots (only when tracking was enabled on a full run).
            let orphaned = reconcile_unreferenced(snapshot_registry.as_ref(), &unreferenced);
            if summary.is_success() && !(unreferenced == "warn" && orphaned) {
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

/// Apply the `--unreferenced` policy to `.snap` files no test referenced this run. `warn` lists them
/// (and the caller fails the run so CI catches rot); `delete` removes them. Returns whether any orphan
/// was found. A no-op when tracking was off (filtered run / policy `ignore`).
fn reconcile_unreferenced(registry: Option<&prova_core::SnapshotRegistry>, policy: &str) -> bool {
    let Some(reg) = registry else {
        return false;
    };
    let orphans = prova_core::unreferenced_snapshots(reg);
    if orphans.is_empty() {
        return false;
    }
    match policy {
        "delete" => {
            eprintln!(
                "prova: deleting {} unreferenced snapshot(s):",
                orphans.len()
            );
            for p in &orphans {
                let _ = std::fs::remove_file(p);
                eprintln!("  deleted {}", p.display());
            }
        }
        _ => {
            eprintln!(
                "prova: {} unreferenced snapshot(s) (no test referenced them; \
                 `--unreferenced delete` to remove):",
                orphans.len()
            );
            for p in &orphans {
                eprintln!("  {}", p.display());
            }
        }
    }
    true
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
    /// Manifest topologies (`[topologies]`) — name → the plugin factory it exposes. Consumed only by
    /// the `up`/`watch`/list verbs, which desugar each to a `prova.topology` registration.
    topologies: BTreeMap<String, crate::manifest::TopologyDecl>,
    /// Capabilities the project's `prova.lua` registered — carried into the run's `RunConfig` so
    /// `requires` resolution sees the same vocabulary the `must_run` precondition just checked. Per
    /// resolve, so the warm MCP's projects don't share.
    capabilities: prova_core::Capabilities,
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
    if !outcome.linked_plugins.is_empty() {
        eprintln!(
            "prova: IDE annotations linked for {}",
            outcome.linked_plugins.join(", ")
        );
    }
    if outcome.luarc_created {
        eprintln!("prova: wrote .luarc.json (editor IDE support enabled)");
    }
    if outcome.luarc_hint {
        eprintln!("prova: IDE annotations ready — run `prova ide setup` to point .luarc.json at them");
    }
}

/// Build the suites a run executes: first any explicit `[suites.*]` from the manifest (each groups
/// its discovered files under one name + optional setup), then the plain paths — a directory with a
/// `suite.lua` is one suite (files share a state → shared `Scope.Suite`), every other file a
/// singleton. Shared by the CLI run path and MCP mode so both consume one manifest the same way.
/// Resolve a manifest path pattern (relative to `base_dir`) to concrete paths. A `*` makes it a
/// glob — `"**/proofs"` matches every `proofs/` directory at any depth, the multi-crate discovery
/// pattern; anything else is joined literally. Sorted for determinism.
fn expand_pattern(base_dir: &Path, pattern: &str) -> Result<Vec<PathBuf>, String> {
    if !pattern.contains('*') {
        return Ok(vec![base_dir.join(pattern)]);
    }
    let joined = base_dir.join(pattern);
    let g = joined.to_string_lossy();
    let mut out: Vec<PathBuf> = glob::glob(&g)
        .map_err(|e| format!("bad path pattern {pattern:?}: {e}"))?
        .filter_map(Result::ok)
        .collect();
    out.sort();
    Ok(out)
}

fn collect_suites(
    base_dir: &Path,
    declared: &BTreeMap<String, SuiteDecl>,
    paths: &[String],
) -> Result<Vec<Suite>, String> {
    let mut suites: Vec<Suite> = Vec::new();
    for (name, decl) in declared {
        let mut files = Vec::new();
        for p in &decl.paths {
            let found = discover_files(&base_dir.join(p))
                .map_err(|err| format!("suite {name:?}: {p}: {err}"))?;
            files.extend(found);
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
    for arg in paths {
        for dir in expand_pattern(base_dir, arg)? {
            let found = discover_suites(&dir).map_err(|err| format!("{arg}: {err}"))?;
            suites.extend(found);
        }
    }
    Ok(suites)
}

/// Build the engine `RunConfig` every verb shares: the bundled archetect module, the global plugin
/// install root, and each resolved named plugin/namespace (so `require(...)` resolves identically
/// in `run`, `up`/`watch`, and `eval`). Callers layer verb-specific knobs (ports, snapshots,
/// selection) on top.
fn engine_config(
    jobs: usize,
    plugins_resolved: &plugins::ResolvedPlugins,
    home: Option<&Home>,
) -> RunConfig {
    // The ambient plugin dir is declared in the manifest (`[run] plugin_root`) — nothing global,
    // nothing from the environment, nothing from the cwd. Discovery locates `prova.toml`; from there
    // the file names everything, so a reader (or an agent) can audit what a `require` could possibly
    // resolve without knowing a single convention baked into this binary.
    let mut config = RunConfig::new(jobs).with_module(prova_archetect::install);
    if let Some(root) = &plugins_resolved.search_root {
        config = config.with_plugin_root(root.clone());
    }
    // Surface where the project is (`prova.root` / `prova.home`) so repo-local plugins can find
    // repo artifacts. Absent when there is no manifest.
    if let Some(h) = home {
        config = config.with_project(h.dir.clone());
    }
    for (name, path) in &plugins_resolved.named {
        config = config.with_named_plugin(name.clone(), path.clone());
    }
    for (canonical, dir) in &plugins_resolved.namespaces {
        config = config.with_plugin_namespace(canonical.clone(), dir.clone());
    }
    config
}

/// Resolve ad-hoc `--plugin name=source` entries the same way manifest plugins resolve and layer
/// them over `plugins_resolved` (CLI wins over a manifest plugin of the same name).
fn layer_cli_plugins(
    cli_plugins: &[String],
    layout: &dyn SystemLayout,
    sources: &BTreeMap<String, String>,
    plugins_resolved: &mut plugins::ResolvedPlugins,
) -> Result<(), ExitCode> {
    if cli_plugins.is_empty() {
        return Ok(());
    }
    let mut adhoc: BTreeMap<String, manifest::PluginSource> = BTreeMap::new();
    for entry in cli_plugins {
        match entry.split_once('=') {
            Some((name, source)) if !name.is_empty() && !source.is_empty() => {
                adhoc.insert(
                    name.to_string(),
                    manifest::PluginSource::Path(source.to_string()),
                );
            }
            _ => {
                eprintln!("prova: --plugin expects name=source, got {entry:?}");
                return Err(ExitCode::from(2));
            }
        }
    }
    // Ad-hoc `--plugin` entries are always local paths (never git), so the git freshness policy is
    // irrelevant here — a default is fine.
    match plugins::resolve_plugins(
        &adhoc,
        Path::new("."),
        layout,
        sources,
        PROVA_VERSION,
        &plugins::GitFetchOptions::default(),
    ) {
        Ok(resolved) => {
            plugins_resolved.named.extend(resolved.named);
            plugins_resolved.namespaces.extend(resolved.namespaces);
            plugins_resolved.roots.extend(resolved.roots);
        }
        Err(e) => {
            eprintln!("prova: {e}");
            return Err(ExitCode::from(2));
        }
    }
    Ok(())
}

/// Read the home's `prova.toml`, overlay `--profile`, apply env, merge CLI overrides, and resolve
/// declared plugins (fetching git sources into the cache). All paths remain manifest-relative (the
/// caller joins them to the home dir). Returns the resolved run or an exit code on error.
fn resolve_from_manifest(
    home: &Home,
    profile: Option<String>,
    cli_jobs: Option<usize>,
    cli_format: Option<Format>,
    config_override: Option<String>,
    layout: &dyn SystemLayout,
    // Run-scoped git-source overrides: `-U`/`--update` forces updates, `--offline` forbids network.
    // Combined here with the manifest's `[updates]` (interval + force) into the effective policy.
    force_update: bool,
    offline: bool,
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

    // Effective git-source freshness policy: the manifest's `[updates]` interval, and `force` from
    // either the manifest or the CLI `-U`; `--offline` from the CLI.
    let git_opts = plugins::GitFetchOptions {
        force: force_update || resolved.updates.force(),
        offline,
        interval: resolved.updates.interval_duration().map_err(|e| {
            eprintln!("prova: {e}");
            ExitCode::from(2)
        })?,
    };

    // Resolve declared plugins relative to the home directory (git sources fetched into cache).
    let mut plugins_resolved = plugins::resolve_plugins(
        &resolved.plugins,
        &home.dir,
        layout,
        &resolved.sources,
        PROVA_VERSION,
        &git_opts,
    )
    .map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })?;

    // The declared plugin dir, absolutised against the project ROOT (like `paths`, and unlike the
    // home-relative `config`). Nothing is added here: a project scans exactly the one directory its
    // manifest names, so the file answers "where can a plugin come from?" on its own.
    plugins_resolved.search_root = resolved.plugin_root.as_ref().map(|r| home.dir.join(r));

    // The optional `prova.lua` companion — loaded with the manifest, and BEFORE the `must_run`
    // precondition below. That order is the whole reason this is a project-level companion rather
    // than something in `suite.lua`: a capability registered at suite-load time would not exist yet
    // at the moment a profile's guarantee is checked, so `must_run = ["gpu"]` could never work.
    // The companion config file, by precedence: `--config` flag, then `PROVA_CONFIG` env, then the
    // manifest's `config`, then the `prova.lua` default. The flag and env are chiefly for tests.
    let companion_rel = config_override
        .or_else(|| std::env::var("PROVA_CONFIG").ok())
        .or_else(|| resolved.config.clone())
        .unwrap_or_else(|| "prova.lua".to_string());
    let companion = home.dir.join(&companion_rel);
    let capabilities = if companion.is_file() {
        match prova_core::load_project_config(
            &companion,
            &engine_config(1, &plugins_resolved, Some(home)),
        ) {
            Ok(caps) => caps,
            // An error, never a warning: a companion that failed to load would leave every
            // capability it meant to register silently missing, so every gated test would skip and
            // the run would be green. That is the vacuous green, one level out from the suite.
            Err(e) => {
                eprintln!("prova: {e}");
                return Err(ExitCode::from(2));
            }
        }
    } else {
        prova_core::Capabilities::default()
    };

    // `must_run` — the guarantees this context makes, checked BEFORE anything runs.
    //
    // A precondition rather than a post-hoc audit of which skips were forgivable: you learn at
    // second one instead of after a suite has run, and a runner that silently lost its daemon is
    // caught before it wastes the run. Exit 2 (config/environment), not 1 (a test failed) — nothing
    // failed a test here; the environment cannot honor the manifest, and whoever is paged wants
    // those to read differently.
    // A capability is an expression, not just a name (`"docker"`, `"dotnet >= 9"`), and it is parsed
    // by the ENGINE's parser — the same one `requires` uses. One vocabulary, two directions: a test
    // states a need, a context states a guarantee, and they must never disagree about what a string
    // means.
    let where_ = profile.as_deref().unwrap_or("run");
    let mut unmet: Vec<String> = Vec::new();
    for cap in &resolved.must_run {
        match capabilities.expr_status(cap) {
            // Satisfied.
            Ok(None) => {}
            // Unmet: absent, or the wrong version. The reason distinguishes them, because "install
            // docker" and "upgrade dotnet" are different days.
            Ok(Some(reason)) => unmet.push(format!(
                "prova: profile {where_:?} guarantees {cap:?}, but {reason}"
            )),
            // The expression itself is broken — a config error, not an environment one.
            Err(e) => unmet.push(format!("prova: profile {where_:?} declares an {e}")),
        }
    }
    if !unmet.is_empty() {
        for line in &unmet {
            eprintln!("{line}");
        }
        eprintln!(
            "prova: a guaranteed capability is a promise about this environment — an unmet one is a \
             broken environment, not a skipped test. Fix the environment, or drop it from `must_run`."
        );
        return Err(ExitCode::from(2));
    }

    let jobs = cli_jobs.or(resolved.jobs).unwrap_or(1);
    let format = match cli_format {
        Some(f) => f,
        None => match resolved.format.as_deref() {
            Some("json") => Format::Json,
            Some("tap") => Format::Tap,
            None | Some("console") => Format::Console,
            Some(other) => {
                eprintln!(
                    "prova: unknown format {other:?} in manifest (expected console|json|tap)"
                );
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
        topologies: resolved.topologies,
        capabilities,
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

/// The embedded agent skill — versioned with the binary so it can never drift from the features.
const SKILL: &str = include_str!("skill.md");

/// `prova skill` prints the skill; `prova skill --install` writes it into the project's
/// `.claude/skills/prova/SKILL.md` (next to the manifest's project root) so the repo carries it.
fn skill_subcommand(args: Vec<String>) -> ExitCode {
    let install = args.iter().any(|a| a == "--install");
    if let Some(bad) = args.iter().find(|a| *a != "--install") {
        eprintln!("prova: skill: unknown argument {bad:?} (expected --install or nothing)");
        return ExitCode::from(2);
    }
    if !install {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let root = match home::find(&std::env::current_dir().unwrap_or_default()) {
        Ok(Some(h)) => h.editor_root(),
        _ => std::env::current_dir().unwrap_or_default(),
    };
    let dir = root.join(".claude/skills/prova");
    let path = dir.join("SKILL.md");
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, SKILL)) {
        eprintln!("prova: skill: could not write {}: {err}", path.display());
        return ExitCode::from(2);
    }
    println!("wrote {}", path.display());
    ExitCode::SUCCESS
}
