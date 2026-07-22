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
mod learn;
mod manifest;
mod mcp;
mod plugins;
mod report;
mod runstate;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use home::Home;
use manifest::{Manage, Manifest, SuiteDecl};
use prova_core::{
    discover_files, discover_path_with, discover_suites, run_suites, JUnitReporter, JsonReporter,
    MultiReporter, PortMode, Reporter, RunConfig, Suite, SystemLayout, TapReporter,
    XdgSystemLayout,
};

/// One subcommand: its dispatch name, its `--help` lines, and its entry point — ONE row per
/// verb, so a verb cannot exist undocumented (the field is required) and the help text cannot
/// name a verb that doesn't dispatch (the same row does both). See docs/plans/autodidact.md §2.8.
struct Verb {
    name: &'static str,
    /// This verb's lines in `prova --help`, exactly as printed (including indentation).
    help: &'static str,
    run: fn(Vec<String>) -> ExitCode,
}

/// Every subcommand, in `--help` order. The run path (`prova [<file-or-dir>...]`) is the
/// fallback when no verb matches.
const VERBS: &[Verb] = &[
    Verb {
        name: "init",
        help: "  prova init [<key>]        render a catalog archetype into this package (interactive if no key),\n\
               \x20                           then wire LuaLS IDE support\n\
               \x20 prova init --list         list the init catalog: the archetypes prova can scaffold from",
        run: init::run,
    },
    Verb {
        name: "ide",
        help: "  prova ide setup           (re)wire this package's LuaLS support: core stubs + .luarc.json",
        run: ide::run,
    },
    Verb {
        name: "eval",
        help: "  prova eval '<code>'       run a one-shot Lua snippet in the full prova environment and print\n\
               \x20                           the returned value (`-` reads the snippet from stdin)",
        run: eval_subcommand,
    },
    Verb {
        name: "skill",
        help: "  prova skill               print the agent skill (how to drive Prova); --install writes it\n\
               \x20                           to .claude/skills/prova/SKILL.md at the package root",
        run: skill_subcommand,
    },
    Verb {
        name: "learn",
        help: "  prova learn [<topic>]     the topic catalog: progressive disclosure of how Prova works\n\
               \x20                           (no topic lists them; slots render THIS package's facts)",
        run: learn::run,
    },
    Verb {
        name: "mcp",
        help: "  prova mcp                 serve an MCP stdio server whose tools mirror the CLI (run, list, eval)",
        run: mcp::run,
    },
    Verb {
        name: "up",
        help: "  prova up [<topology>] [<url>]  list/stand up a topology — local, or from a git repo that advertises it",
        run: up_subcommand,
    },
    Verb {
        name: "watch",
        help: "  prova watch <topology>    stand up a topology and re-apply on definition change (dev loop)",
        run: watch_subcommand,
    },
    Verb {
        name: "start",
        help: "  prova start <topology>    stand up a topology detached (returns; use `down` to stop)",
        run: start_subcommand,
    },
    Verb {
        name: "down",
        help: "  prova down <topology>     tear down a detached topology",
        run: down_subcommand,
    },
    Verb {
        name: "ps",
        help: "  prova ps                  list running topologies",
        run: ps_subcommand,
    },
    Verb {
        name: "plugin",
        help: "  prova plugin lint <f>...  check plugin files against the namespacing grammar",
        run: plugin_subcommand,
    },
];

/// `prova --help`, assembled from the verb table so the two cannot disagree.
fn help_text() -> String {
    let verbs: Vec<&str> = VERBS.iter().map(|v| v.help).collect();
    format!(
        "usage:\n\
         \x20 prova <file-or-dir>...    run the given files/dirs\n\
         \x20 prova                     run the suite declared in prova.toml (found by walking up)\n\
         {}\n\n{OPTIONS}",
        verbs.join("\n")
    )
}

const OPTIONS: &str = "\
options:
  -p, --profile NAME        run a profile from the manifest
      --manifest PATH       use a specific manifest (default ./prova.toml)
      --format console|json|tap  output format (--json is shorthand)
      --color auto|always|never  color console output (default auto: TTY only; honors NO_COLOR)
  -q, --quiet               only print failures, the recap, and the summary
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
    // Subcommands dispatch through the verb table; everything else is the run path.
    let mut raw = std::env::args().skip(1).peekable();
    if let Some(first) = raw.peek() {
        if let Some(verb) = VERBS.iter().find(|v| v.name == *first) {
            raw.next();
            return (verb.run)(raw.collect());
        }
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
/// `require("postgres")` works from a package directory; without a manifest it still runs with the
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
    let mut positionals: Vec<String> = Vec::new();
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
                    "usage: prova up [<topology>] [<git-url>] [--fixed] [--profile NAME] [--manifest PATH]\n\
                     \n\
                     with no topology, list the topologies this package defines.\n\
                     with one, stand it up (declared with prova.topology) and hold it running until\n\
                     Ctrl-C, printing each resource's endpoint.\n\
                     \n\
                     with a git URL, act on a REMOTE repo that advertises topologies instead of the\n\
                     local package: `prova up <url>` lists what it advertises; `prova up <topology>\n\
                     <url>` stands that one up. The repo is fetched and pinned like a git plugin.\n\
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
            other if positionals.len() < 2 => positionals.push(other.to_string()),
            other => {
                eprintln!("prova up: unexpected argument {other:?}");
                return ExitCode::from(2);
            }
        }
    }

    // Dispatch on the positionals. A git URL routes to the remote forms; otherwise it's a local name.
    //   prova up                    → list local topologies
    //   prova up <topology>         → stand up a local topology
    //   prova up <url>              → list a repo's advertised topologies
    //   prova up <topology> <url>   → stand up a repo's advertised topology
    let (name, url): (Option<String>, Option<String>) = match positionals.as_slice() {
        [] => (None, None),
        [a] if plugins::is_git_source(a) => (None, Some(a.clone())),
        [a] => (Some(a.clone()), None),
        [a, b] if plugins::is_git_source(b) => (Some(a.clone()), Some(b.clone())),
        [a, b] => {
            eprintln!("prova up: expected `<topology> <url>`, but {b:?} is not a git source (and {a:?} was already given)");
            return ExitCode::from(2);
        }
        _ => unreachable!("capped at 2 positionals"),
    };

    if let Some(url) = url {
        return up_from_git(name.as_deref(), &url, fixed);
    }

    // No name → the discovery form: list what's defined (like `prova init` listing templates).
    let Some(name) = name else {
        return up_list(profile, manifest_path);
    };

    let prep = match build_topology_run("up", Some(&name), profile, manifest_path, fixed) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // Gate on the topology's declared environment BEFORE provisioning: a missing capability should
    // stop us early with a clear reason, not fail deep in a factory (or, for a factory that needs
    // nothing, hold a topology the environment can't really support).
    if let Err(code) = check_topology_requires(&prep, &name) {
        return code;
    }
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

/// `prova up` with no name — list the topologies this package defines, so you can see what's there
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

/// The git forms: `prova up <url>` lists a repo's advertised topologies; `prova up <topology> <url>`
/// stands one up. The repo is fetched (pinned, freshness-gated) like a git `[plugins]` source, its
/// `[[plugin.topologies]]` advertisement is read, and a standalone engine registers the chosen
/// topology — no local prova package required.
fn up_from_git(name: Option<&str>, url: &str, fixed: bool) -> ExitCode {
    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("prova up: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };
    eprintln!("prova: fetching {url}…");
    let src = match plugins::fetch_topology_source(
        url,
        &layout,
        PROVA_VERSION,
        &plugins::GitFetchOptions::default(),
    ) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("prova up: {err}");
            return ExitCode::from(2);
        }
    };

    // No topology → list what the repo advertises.
    let Some(name) = name else {
        if src.advertised.is_empty() {
            eprintln!(
                "prova up: {url} advertises no topologies (no [[plugin.topologies]] in its prova.toml)"
            );
            return ExitCode::from(2);
        }
        println!("topologies advertised by {url} ({}):", src.advertised.len());
        for t in &src.advertised {
            println!("  {}", t.name);
        }
        println!("\nstand one up with `prova up <topology> {url}`.");
        return ExitCode::SUCCESS;
    };

    // Named → find the advertised topology, gate on its requires, stand it up.
    let Some(adv) = src.advertised.iter().find(|a| a.name == name) else {
        let names: Vec<&str> = src.advertised.iter().map(|a| a.name.as_str()).collect();
        eprintln!(
            "prova up: {url} advertises no topology {name:?} (has: {})",
            names.join(", ")
        );
        return ExitCode::from(2);
    };

    // Environment gate — built-in capabilities only (a remote `up` has no local companion to register
    // package-specific ones).
    let caps = prova_core::Capabilities::default();
    for req in &adv.requires {
        match caps.expr_status(req) {
            Ok(None) => {}
            Ok(Some(reason)) => {
                eprintln!("prova up: cannot stand up topology {name:?}: it requires {reason}");
                return ExitCode::from(2);
            }
            Err(err) => {
                eprintln!("prova up: topology {name:?}: invalid requires {req:?}: {err}");
                return ExitCode::from(2);
            }
        }
    }

    let config = engine_config(1, &src.plugins, None)
        .with_ports(if fixed {
            PortMode::Fixed
        } else {
            PortMode::Auto
        })
        .with_topology_registration(name, &src.require_name, &adv.factory, None);

    eprintln!("prova: standing up topology {name:?} from {url}…");
    let result = prova_core::up(&[], name, &config, |endpoints| {
        print_endpoints(name, endpoints);
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

/// Everything the `up`/`watch` verbs need to stand a topology up: the located package, the files that
/// may declare topologies, and the engine config (plugins resolved, port mode set).
struct TopologyRun {
    home: Home,
    files: Vec<PathBuf>,
    config: RunConfig,
    /// Each manifest topology's effective `requires` (advertisement + registration), keyed by name —
    /// checked against `capabilities` before `up` provisions it.
    topology_requires: BTreeMap<String, Vec<String>>,
    capabilities: prova_core::Capabilities,
}

/// Serialize a `[topologies].<name>.options` table into a Lua table-literal expression, so the
/// registration can hand it to the factory as `factory(ctx, <literal>)`. Only literal values are
/// emitted — strings are quoted-and-escaped, keys use `["k"]` form so any key is legal — so the
/// result is a self-contained value that can never inject code. `None` for an empty table (register
/// the factory bare).
fn topology_options_to_lua(options: &toml::Table) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    Some(toml_value_to_lua(&toml::Value::Table(options.clone())))
}

fn toml_value_to_lua(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => lua_quote(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => lua_quote(&d.to_string()),
        toml::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(toml_value_to_lua).collect();
            format!("{{ {} }}", parts.join(", "))
        }
        toml::Value::Table(t) => {
            let parts: Vec<String> = t
                .iter()
                .map(|(k, val)| format!("[{}] = {}", lua_quote(k), toml_value_to_lua(val)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// A Lua double-quoted string literal with the metacharacters escaped.
fn lua_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
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

    // Locate the package (the manifest tells us where topologies + plugins live).
    let home = resolve_home(manifest_path.as_deref())?;

    let run = resolve_from_manifest(&home, profile, None, None, None, &layout, false, false)?;

    // Gather every file that could declare a topology: the discovered `proofs` dirs plus any explicit
    // suites. `proofs` are directory-NAME patterns found anywhere below the root; suites are literal.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut discover = |dir: PathBuf| -> Result<(), ExitCode> {
        match discover_files(&dir) {
            Ok(found) => {
                files.extend(found);
                Ok(())
            }
            Err(err) => {
                eprintln!("prova {verb}: {}: {err}", dir.display());
                Err(ExitCode::from(2))
            }
        }
    };
    for dir in find_proof_dirs(&home.dir, &run.proofs) {
        discover(dir)?;
    }
    for decl in run.suites.values() {
        for p in &decl.paths {
            discover(home.dir.join(p))?;
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
    // after the files — so `prova up <name>` and the listing form see them as first-class. The factory
    // is either given directly or resolved from the plugin's advertised set (`[[plugin.topologies]]`),
    // whose `requires` (plus the registration's) become the topology's environment gate.
    let mut topology_requires: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (alias, decl) in &run.topologies {
        let resolved = match plugins::resolve_topology(alias, decl, &run.plugins) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("prova {verb}: {e}");
                return Err(ExitCode::from(2));
            }
        };
        let options = topology_options_to_lua(&decl.options);
        config = config.with_topology_registration(alias, &decl.plugin, resolved.factory, options);
        if !resolved.requires.is_empty() {
            topology_requires.insert(alias.clone(), resolved.requires);
        }
    }

    Ok(TopologyRun {
        home,
        files,
        config,
        topology_requires,
        capabilities: run.capabilities,
    })
}

/// Reject standing up `name` when its `requires` are not met here — before anything is provisioned.
/// `Ok(())` = clear to proceed (met, or the topology declares nothing); `Err` = the reason, already
/// printed. Only manifest topologies carry `requires` today; a Lua-declared one has none.
fn check_topology_requires(prep: &TopologyRun, name: &str) -> Result<(), ExitCode> {
    let Some(requires) = prep.topology_requires.get(name) else {
        return Ok(());
    };
    for req in requires {
        match prep.capabilities.expr_status(req) {
            Ok(None) => {}
            Ok(Some(reason)) => {
                eprintln!("prova up: cannot stand up topology {name:?}: it requires {reason}");
                return Err(ExitCode::from(2));
            }
            Err(e) => {
                eprintln!("prova up: topology {name:?}: invalid requires {req:?}: {e}");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(())
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

/// Locate the prova package home from `--manifest` or by walking up from the current directory.
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

/// `prova ps` — list this package's running topologies and their endpoints. Stale records (holder
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
    let mut cli_color: Option<report::ColorMode> = None;
    let mut cli_quiet = false;
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
        // `--color auto|always|never`: color the console output (auto = only on a terminal).
        if let Some(v) = value_flag(&arg, &mut args, &["--color"]) {
            match report::ColorMode::parse(&v) {
                Some(mode) => cli_color = Some(mode),
                None => {
                    eprintln!("prova: unknown --color {v:?} (expected auto|always|never)");
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
            "--quiet" | "-q" => cli_quiet = true,
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
                println!("{}", help_text());
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

    // Resolve the run. Explicit path args bypass the manifest (literal paths relative to cwd, no IDE
    // management); otherwise read the home's `prova.toml` (a `proofs` name-pattern rooted at home).
    let from_manifest = explicit_paths.is_empty();
    let (
        base_dir,
        paths,
        jobs,
        format,
        manifest_color,
        manifest_quiet,
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
            None,
            None,
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
                // `home.dir` IS the package root (the parent of a nested `.prova/`/`prova/` nook), so
                // proof patterns, `config`, and `plugin_root` all resolve against it. `proofs/` lives
                // at the root while prova's own files tuck into the nook.
                home.dir.clone(),
                r.proofs,
                r.jobs,
                r.format,
                r.color,
                r.quiet,
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
    let suites = match collect_suites(&base_dir, &declared, &paths, from_manifest) {
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

    // Color resolution, per key: CLI flag > `PROVA_COLOR` env > manifest > auto. Format never
    // auto-switches (a piped console run stays console, just uncolored); only color detects.
    let color = cli_color
        .or_else(|| {
            std::env::var("PROVA_COLOR")
                .ok()
                .and_then(|v| report::ColorMode::parse(&v))
        })
        .or(manifest_color)
        .unwrap_or(report::ColorMode::Auto);
    // `--quiet` can only *enable* — a flag that silences must not be silently un-silenced.
    let quiet = cli_quiet || manifest_quiet.unwrap_or(false);
    // Displayed source locations relativize against the package root (else the cwd).
    let rel_root = home
        .as_ref()
        .map(|h| h.dir.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    // The stdout sink chosen by --format, plus an optional JUnit XML *file* sink (--junit), fanned
    // out through a MultiReporter so a CI run can print to the console and drop a results.xml at once.
    let mut sinks: Vec<Box<dyn Reporter>> = vec![match format {
        Format::Console => Box::new(report::HumanReporter::new(color, quiet, rel_root)),
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
    proofs: Vec<String>,
    jobs: usize,
    format: Format,
    /// Manifest `color`/`quiet` (pre-parsed) — the CLI flags and `PROVA_COLOR` env override them
    /// at the wiring site.
    color: Option<report::ColorMode>,
    quiet: Option<bool>,
    suites: BTreeMap<String, SuiteDecl>,
    plugins: plugins::ResolvedPlugins,
    sources: BTreeMap<String, String>,
    manage: Manage,
    /// Manifest topologies (`[topologies]`) — name → the plugin factory it exposes. Consumed only by
    /// the `up`/`watch`/list verbs, which desugar each to a `prova.topology` registration.
    topologies: BTreeMap<String, crate::manifest::TopologyDecl>,
    /// Capabilities the package's `prova.lua` registered — carried into the run's `RunConfig` so
    /// `requires` resolution sees the same vocabulary the `must_run` precondition just checked. Per
    /// resolve, so the warm MCP's packages don't share.
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
        eprintln!(
            "prova: IDE annotations ready — run `prova ide setup` to point .luarc.json at them"
        );
    }
}

/// Build the suites a run executes: first any explicit `[suites.*]` from the manifest (each groups
/// its discovered files under one name + optional setup), then the plain paths — a directory with a
/// `suite.lua` is one suite (files share a state → shared `Scope.Suite`), every other file a
/// singleton. Shared by the CLI run path and MCP mode so both consume one manifest the same way.
/// Resolve a manifest path pattern (relative to `base_dir`) to concrete paths. A `*` makes it a
/// glob — `"**/proofs"` matches every `proofs/` directory at any depth, the multi-crate discovery
/// pattern; anything else is joined literally. Sorted for determinism.
/// Directory names prova never descends into when matching `[run] proofs` patterns: its own nook
/// (`prova`/`.prova`), any hidden dir (VCS metadata, tool caches), and common build/dependency trees.
/// A plugin's own `proofs/` lives under the `.prova/` nook, so this is what keeps a dependency's
/// proofs out of the consuming package's run.
fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "prova" | "target" | "node_modules" | "vendor" | "dist" | "build" | "testdata"
        )
}

/// Whether a directory basename matches one of the `[run] proofs` patterns — a glob when the pattern
/// carries a metacharacter, an exact-name match otherwise.
fn name_matches(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        if p.contains(['*', '?', '[']) {
            glob::Pattern::new(p).map(|g| g.matches(name)).unwrap_or(false)
        } else {
            p == name
        }
    })
}

/// Every directory below `root` whose name matches a `proofs` pattern — the discovery model for
/// `[run] proofs`. Walks the tree (skipping prova's nook, hidden dirs, and build trees) and PRUNES at
/// a match: a matched `proofs/` owns its whole subtree (handed to `discover_suites`), so a `proofs/`
/// nested inside it is not matched again. Sorted for deterministic order.
fn find_proof_dirs(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    fn walk(dir: &Path, patterns: &[String], out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subdirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();
        for path in subdirs {
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_skipped_dir(name) {
                continue;
            }
            if name_matches(name, patterns) {
                out.push(path); // prune — the subtree is this suite's, not re-scanned for `proofs/`
            } else if crate::home::has_manifest(&path) {
                continue; // a nested, independent package — its proofs are its own, not ours
            } else {
                walk(&path, patterns, out);
            }
        }
    }
    let mut out = Vec::new();
    // `"."` is the flat escape hatch: discover the whole tree from the root itself (for a package
    // whose proofs are not tucked under a named directory). It composes with name patterns.
    if patterns.iter().any(|p| p == ".") {
        out.push(root.to_path_buf());
    }
    walk(root, patterns, &mut out);
    out.sort();
    out.dedup();
    out
}

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
    proofs: &[String],
    patterns: bool,
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
    if patterns {
        // Manifest `[run] proofs`: each entry is a directory-NAME pattern found anywhere below the
        // package root.
        for dir in find_proof_dirs(base_dir, proofs) {
            let found = discover_suites(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            suites.extend(found);
        }
    } else {
        // Explicit `prova <path>...` args: literal files/dirs (with glob support), relative to cwd.
        for arg in proofs {
            for dir in expand_pattern(base_dir, arg)? {
                let found = discover_suites(&dir).map_err(|err| format!("{arg}: {err}"))?;
                suites.extend(found);
            }
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
    // Surface where the package is (`prova.root` / `prova.home`) so repo-local plugins can find
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
    // Each plugin's `library/*.lua` stubs feed `prova.help()` — the plugin documents itself once
    // and the IDE, help(), and MCP introspect all answer from the same files.
    for root in plugins_resolved.roots.values() {
        config = config.with_help_root(root.clone());
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
#[allow(clippy::too_many_arguments)] // the run's independent axes; a params struct would just rename them
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
    if resolved.proofs.is_empty() && resolved.suites.is_empty() {
        eprintln!(
            "prova: manifest {} defines no proofs or suites to run",
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

    // Quietly reap plugin source trees unused past the retention window (throttled to ~daily). The
    // run's own trees are leased, so they're never reaped mid-run.
    let retention = resolved
        .updates
        .retention_duration()
        .unwrap_or(manifest::UpdatesSection::DEFAULT_RETENTION);
    plugins::prune_plugin_cache(layout, retention);

    // The declared plugin dir, absolutised against the package ROOT (like `paths`, and unlike the
    // home-relative `config`). Nothing is added here: a package scans exactly the one directory its
    // manifest names, so the file answers "where can a plugin come from?" on its own.
    plugins_resolved.search_root = resolved.plugin_root.as_ref().map(|r| home.dir.join(r));

    // The optional `prova.lua` companion — loaded with the manifest, and BEFORE the `must_run`
    // precondition below. That order is the whole reason this is a package-level companion rather
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
    let color = match resolved.color.as_deref() {
        None => None,
        Some(s) => match report::ColorMode::parse(s) {
            Some(mode) => Some(mode),
            None => {
                eprintln!("prova: unknown color {s:?} in manifest (expected auto|always|never)");
                return Err(ExitCode::from(2));
            }
        },
    };
    Ok(ManifestRun {
        proofs: resolved.proofs,
        jobs,
        format,
        color,
        quiet: resolved.quiet,
        suites: resolved.suites,
        plugins: plugins_resolved,
        sources: resolved.sources,
        manage,
        topologies: resolved.topologies,
        capabilities,
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // the helpers below are shared with mcp.rs, not test-only
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

    /// Collect every verb a document tells the agent to RUN — the word after "prova " in
    /// backticked/fenced command position — skipping non-verb shapes: flags (`prova --list`),
    /// placeholders (`prova <verb>`), file arguments, and `prova.toml`-style dotted names.
    /// Plain-prose "prova" (the product name) is not a command and is not linted.
    fn verbs_uttered(doc: &str) -> Vec<String> {
        let mut chunks: Vec<&str> = doc.split("`prova ").skip(1).collect();
        // Fenced examples put commands at line start with no inline backticks.
        chunks.extend(
            doc.lines()
                .map(str::trim_start)
                .filter_map(|l| l.strip_prefix("prova ")),
        );
        let mut out = Vec::new();
        for chunk in chunks {
            let word: String = chunk
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !word.is_empty()
                && word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && !chunk.starts_with(&format!("{word}."))
            {
                out.push(word);
            }
        }
        out
    }

    /// The reference lint (docs/plans/autodidact.md §2.6.7): every verb the skill and the topics
    /// tell an agent to run must exist in the verb table — the docs cannot advertise a command
    /// the binary would reject.
    #[test]
    fn skill_and_topics_only_name_real_verbs() {
        let known: std::collections::BTreeSet<&str> =
            VERBS.iter().map(|v| v.name).collect();
        // Words that follow `prova ` without being subcommand verbs: run-path/general usage.
        let non_verbs = ["run", "package", "environment", "release"];
        let docs: Vec<(&str, &str)> = std::iter::once(("skill.md", SKILL))
            .chain(
                learn::Topic::ALL
                    .iter()
                    .map(|t| (t.key(), t.rendered_source_for_lint())),
            )
            .collect();
        for (name, doc) in docs {
            for verb in verbs_uttered(doc) {
                assert!(
                    known.contains(verb.as_str()) || non_verbs.contains(&verb.as_str()),
                    "{name} tells the agent to run `prova {verb}`, which is not a verb the \
                     binary dispatches — fix the doc or add the verb to VERBS"
                );
            }
        }
    }

    /// Every verb's help text names the verb it dispatches — the row documents itself.
    #[test]
    fn every_verb_documents_itself() {
        for verb in VERBS {
            assert!(
                verb.help.contains(&format!("prova {}", verb.name)),
                "verb `{}` has help text that never mentions it",
                verb.name
            );
        }
        // And the assembled help is the verbs, in order.
        let help = help_text();
        for verb in VERBS {
            assert!(help.contains(verb.help), "help_text() dropped `{}`", verb.name);
        }
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

/// `prova skill` prints the skill; `prova skill --install` writes it into the package's
/// `.claude/skills/prova/SKILL.md` (next to the manifest's package root) so the repo carries it.
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
        Ok(Some(h)) => h.dir,
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
