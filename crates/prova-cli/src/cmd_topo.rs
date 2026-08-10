//! The topology verbs: up, watch, start, down, ps — and their shared plumbing.

use super::*;

/// What `topology_flags` yields: the positionals, `--profile`, `--manifest`, and `--fixed`.
type TopologyFlags = (Vec<String>, Option<String>, Option<String>, bool);

/// The topology verbs' shared flag loop: `--profile`/`-p`, `--manifest`, `--fixed`, the verb's
/// help text, and up to `max_positionals` bare arguments — each verb interprets its own.
/// `Err(ExitCode::SUCCESS)` is the `--help` early exit.
fn topology_flags(
    verb: &str,
    usage: &str,
    max_positionals: usize,
    args: Vec<String>,
) -> Result<TopologyFlags, ExitCode> {
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
            "--fixed" => fixed = true,
            "-h" | "--help" => {
                println!("{usage}");
                return Err(ExitCode::SUCCESS);
            }
            other if other.starts_with('-') => {
                eprintln!("prova {verb}: unknown flag {other}");
                return Err(ExitCode::from(2));
            }
            other if positionals.len() < max_positionals => positionals.push(other.to_string()),
            other => {
                eprintln!("prova {verb}: unexpected argument {other:?}");
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok((positionals, profile, manifest_path, fixed))
}

/// `prova up <topology>` — stand up a named topology (the same definition tests use) and hold it
/// running until Ctrl-C, printing each resource's endpoint. Discovers the topology in the manifest's
/// test files, resolves declared plugins, and hands off to the engine's held-execution mode.
pub(crate) fn up_subcommand(args: Vec<String>) -> ExitCode {
    const USAGE: &str = "usage: prova up [<topology>] [<git-url>] [--fixed] [--profile NAME] [--manifest PATH]\n\
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
        \x20        Only one fixed instance of a port can run at a time.";
    let (positionals, profile, manifest_path, fixed) =
        match topology_flags("up", USAGE, 2, args) {
            Ok(parsed) => parsed,
            Err(code) => return code,
        };

    // Dispatch on the positionals. A git URL routes to the remote forms; otherwise it's a local name.
    //   prova up                    → list local topologies
    //   prova up <topology>         → stand up a local topology
    //   prova up <url>              → list a repo's advertised topologies
    //   prova up <topology> <url>   → stand up a repo's advertised topology
    let (name, url): (Option<String>, Option<String>) = match positionals.as_slice() {
        [] => (None, None),
        [a] if packages::is_git_source(a) => (None, Some(a.clone())),
        [a] => (Some(a.clone()), None),
        [a, b] if packages::is_git_source(b) => (Some(a.clone()), Some(b.clone())),
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
    let result = prova_core::up(&files, &name, &config, |endpoints, snapshot| {
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
            value: snapshot.clone(),
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
pub(crate) fn up_list(profile: Option<String>, manifest_path: Option<String>) -> ExitCode {
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
pub(crate) fn up_from_git(name: Option<&str>, url: &str, fixed: bool) -> ExitCode {
    let layout = match XdgSystemLayout::new() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("prova up: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };
    eprintln!("prova: fetching {url}…");
    let src = match packages::fetch_topology_source(
        url,
        &layout,
        PROVA_VERSION,
        &packages::GitFetchOptions::default(),
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
                "prova up: {url} advertises no topologies (no [[package.topologies]] in its prova.toml)"
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

    let config = engine_config(1, &src.dependencies, None, prova_core::progress::null())
        .with_ports(if fixed {
            PortMode::Fixed
        } else {
            PortMode::Auto
        })
        .with_topology_registration(name, &src.require_name, &adv.factory, None);

    eprintln!("prova: standing up topology {name:?} from {url}…");
    let result = prova_core::up(&[], name, &config, |endpoints, _snapshot| {
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
pub(crate) struct TopologyRun {
    pub(crate) home: Home,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) config: RunConfig,
    /// Each manifest topology's effective `requires` (advertisement + registration), keyed by name —
    /// checked against `capabilities` before `up` provisions it.
    pub(crate) topology_requires: BTreeMap<String, Vec<String>>,
    pub(crate) capabilities: prova_core::Capabilities,
}

/// Serialize a `[topologies].<name>.options` table into a Lua table-literal expression, so the
/// registration can hand it to the factory as `factory(ctx, <literal>)`. Only literal values are
/// emitted — strings are quoted-and-escaped, keys use `["k"]` form so any key is legal — so the
/// result is a self-contained value that can never inject code. `None` for an empty table (register
/// the factory bare).
pub(crate) fn topology_options_to_lua(options: &toml::Table) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    Some(toml_value_to_lua(&toml::Value::Table(options.clone())))
}

pub(crate) fn toml_value_to_lua(v: &toml::Value) -> String {
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
pub(crate) fn lua_quote(s: &str) -> String {
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
pub(crate) fn build_topology_run(
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

    let run = resolve_from_manifest(&home, profile, None, None, None, &layout, false, false, true)?;

    // The inhabited verbs resolve topologies from `[topologies]` REGISTRATIONS ONLY — never by
    // scanning code. A topology has exactly two consumers and they enter by different doors: a test
    // builds one in-process (`prova.topology(...)`, requiring what it needs), and `up`/`down` and
    // friends stand up a registered factory. Nothing discovers one by reading test files.
    //
    // Previously `up` loaded every proof file and picked up any `prova.topology` call it found. That
    // made a test-local fixture silently addressable as an environment, and it collided head-on with
    // the manifest: registering a topology AND declaring it in a proof — the natural thing to do when
    // one package is both a plugin and its own suite — aborted the run with
    // `topology "x" is already defined`, an error that never mentioned that one of the two came from
    // the manifest. Sourcing from one place removes the ambiguity rather than papering over it.
    //
    // It also makes the `requires` gate universal: every topology now carries the advertisement's
    // environment requirements, where a code-declared one had none and stood up ungated.
    //
    // No files are loaded, so a `[topologies]` entry is the whole surface.
    let files: Vec<PathBuf> = Vec::new();
    if run.topologies.is_empty() {
        match name {
            Some(n) => eprintln!(
                "prova {verb}: no topology {n:?} — register it in [topologies], e.g.\n  \
                 [topologies]\n  {n} = {{ package = \"<package>\", topology = \"<advertised>\" }}"
            ),
            None => eprintln!(
                "prova {verb}: no topologies registered — add a [topologies] entry to prova.toml"
            ),
        }
        return Err(ExitCode::from(2));
    }
    if let Some(n) = name {
        if !run.topologies.contains_key(n) {
            let known: Vec<&str> = run.topologies.keys().map(String::as_str).collect();
            eprintln!(
                "prova {verb}: no topology {n:?} in [topologies] (registered: {})",
                known.join(", ")
            );
            return Err(ExitCode::from(2));
        }
    }

    // Build the engine config with the declared plugins (so the topology's `require(...)` resolves).
    // `--fixed` pins ports for external reachability; the default is random (like tests), so several
    // topologies can be inhabited at once without colliding.
    let mut config = engine_config(1, &run.dependencies, Some(&home), progress::sink(progress::Mode::Auto))
        .with_ports(if fixed {
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
        let resolved = match packages::resolve_topology(alias, decl, &run.dependencies) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("prova {verb}: {e}");
                return Err(ExitCode::from(2));
            }
        };
        let options = topology_options_to_lua(&decl.options);
        config = config.with_topology_registration(alias, &decl.package, resolved.factory, options);
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
/// printed. Every topology reaching this point is a `[topologies]` registration, so the gate is
/// universal — there is no longer a code-declared topology that stands up ungated.
pub(crate) fn check_topology_requires(prep: &TopologyRun, name: &str) -> Result<(), ExitCode> {
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
pub(crate) fn watch_subcommand(args: Vec<String>) -> ExitCode {
    const USAGE: &str = "usage: prova watch <topology> [--fixed] [--profile NAME] [--manifest PATH]\n\
        \n\
        stand up a topology and re-provision it whenever its definition files change,\n\
        holding until Ctrl-C. A live dev loop over the same definition your tests use.\n\
        \n\
        --fixed  keep endpoints on canonical ports so they stay stable across re-applies.";
    let (mut positionals, profile, manifest_path, fixed) =
        match topology_flags("watch", USAGE, 1, args) {
            Ok(parsed) => parsed,
            Err(code) => return code,
        };
    let Some(name) = positionals.pop() else {
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
pub(crate) fn resolve_home(manifest_path: Option<&str>) -> Result<Home, ExitCode> {
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
pub(crate) fn print_endpoints(name: &str, endpoints: &[prova_core::Endpoint]) {
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
pub(crate) fn start_subcommand(args: Vec<String>) -> ExitCode {
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
pub(crate) fn down_subcommand(args: Vec<String>) -> ExitCode {
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
pub(crate) fn ps_subcommand(args: Vec<String>) -> ExitCode {
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
pub(crate) fn parse_topology_args(
    verb: &str,
    args: Vec<String>,
) -> Result<(String, Option<String>, Option<String>, bool), ExitCode> {
    let usage =
        format!("usage: prova {verb} <topology> [--fixed] [--profile NAME] [--manifest PATH]");
    let (mut positionals, profile, manifest_path, fixed) =
        topology_flags(verb, &usage, 1, args)?;
    match positionals.pop() {
        Some(n) => Ok((n, manifest_path, profile, fixed)),
        None => {
            eprintln!("usage: prova {verb} <topology>");
            Err(ExitCode::from(2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The shared loop: flags land in their slots, positionals cap at the verb's budget, and
    /// `--help`/unknown flags exit early (success and usage-error respectively — ExitCode carries
    /// which; here we assert the parse never returns Ok for them).
    #[test]
    fn topology_flags_parses_and_caps() {
        let (pos, profile, manifest, fixed) =
            topology_flags("up", "usage", 2, args(&["alpha", "-p", "ci", "--fixed", "beta"]))
                .unwrap();
        assert_eq!(pos, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(profile.as_deref(), Some("ci"));
        assert_eq!(manifest, None);
        assert!(fixed);

        assert!(topology_flags("watch", "usage", 1, args(&["a", "b"])).is_err(), "over the cap");
        assert!(topology_flags("up", "usage", 2, args(&["--bogus"])).is_err(), "unknown flag");
        assert!(topology_flags("up", "usage", 2, args(&["--help"])).is_err(), "help exits early");
    }

    /// start/down's wrapper: exactly one name, required.
    #[test]
    fn parse_topology_args_requires_the_name() {
        let (name, manifest, profile, fixed) =
            parse_topology_args("start", args(&["kafka", "--manifest", "m.toml"])).unwrap();
        assert_eq!(name, "kafka");
        assert_eq!(manifest.as_deref(), Some("m.toml"));
        assert_eq!(profile, None);
        assert!(!fixed);
        assert!(parse_topology_args("down", args(&[])).is_err(), "no name is a usage error");
    }
}
