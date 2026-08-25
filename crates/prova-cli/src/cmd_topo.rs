//! The topology verbs: up, start, down, ps — and their shared plumbing.

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

    if let Err(code) = guard_double_provision("up", &home, &name) {
        return code;
    }

    eprintln!("prova: standing up topology {name:?}…");
    // Register BEFORE the factory runs (docs/design/agent-ergonomics.md#starting-is-a-visible-state).
    // A record that appeared only on success made a topology *coming* up invisible: `prova ps` said
    // "no topologies running" while the machine was busy creating a cluster, `prova down` had nothing
    // to stop, and a second `start` sailed past the guard straight into `kind create`. The starting
    // record carries no endpoints and a null `value` — it is a claim on the NAME, not an environment
    // anything may bind to, which is why `attach` skips it (see `attachable`).
    let starting = runstate::Record {
        name: name.clone(),
        pid: std::process::id(),
        started_at: runstate::now_secs(),
        status: runstate::Status::Starting,
        endpoints: Vec::new(),
        value: serde_json::Value::Null,
    };
    if let Err(e) = runstate::write(&home, &starting) {
        eprintln!("prova up: could not record run-state: {e}");
    }

    let state_home = home.clone();
    let state_name = name.clone();
    let result = prova_core::up(&files, &name, &config, |endpoints, snapshot| {
        // Flip to ready, now that endpoints and the rehydration payload are real.
        let record = runstate::Record {
            name: state_name.clone(),
            pid: std::process::id(),
            started_at: runstate::now_secs(),
            status: runstate::Status::Ready,
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

/// Refuse to double-provision a name something else already holds — the one guard `up` and `start`
/// share, and the one `watch` never had.
///
/// Two states refuse, not one (docs/design/agent-ergonomics.md#second-start-joins-or-refuses). A
/// **ready** holder is the obvious case. A **starting** one is the case that bit: before the record
/// was written at birth, a topology mid-`kind create` was invisible here, so a second invocation
/// walked past this check and into the factory, where it met kind's own
/// `node(s) already exist for a cluster with the name "…"` — a collision reported by the tool being
/// driven, three layers from the cause.
///
/// A dead record is stale and cleared, but the two states clear differently
/// (docs/design/agent-ergonomics.md#a-stale-starting-record-implies-residue). A dead *ready* holder
/// got as far as being up, so its teardown either ran or its resources are the user's known problem.
/// A dead *starting* holder was killed with the factory half-done, and removing its record deletes
/// the evidence without deleting the half-built cluster — so the next attempt fails on a collision
/// whose real cause is the previous crash. We proceed either way (we cannot know what a factory
/// created), but the starting case says what may be lying around, because a port conflict that is
/// really residue sends people to diagnose networking.
fn guard_double_provision(verb: &str, home: &Home, name: &str) -> Result<(), ExitCode> {
    let Some(rec) = runstate::read(home, name) else {
        return Ok(());
    };
    if runstate::is_alive(rec.pid) {
        let held = runstate::now_secs().saturating_sub(rec.started_at);
        match rec.status {
            runstate::Status::Ready => eprintln!(
                "prova {verb}: topology {name:?} is already up (pid {}, up {held}s)",
                rec.pid
            ),
            runstate::Status::Starting => eprintln!(
                "prova {verb}: topology {name:?} is already starting (pid {}, {held}s in) — wait for \
                 it, watch {}, or `prova down {name}` to stop it",
                rec.pid,
                runstate::log_path(home, name).display()
            ),
        }
        return Err(ExitCode::from(2));
    }
    if rec.status == runstate::Status::Starting {
        eprintln!(
            "prova {verb}: clearing a stale STARTING record for {name:?} (pid {} is gone) — that \
             holder died with its factory half-run, so whatever it had already created is still \
             out there and unowned. If this attempt fails on a name or port that is already taken, \
             that is the residue, not a new conflict.",
            rec.pid
        );
    }
    runstate::remove(home, name);
    Ok(())
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

/// Everything the `up` verbs need to stand a topology up: the located package, the files that
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
/// verb (`up`, and its listing form). `verb` only labels errors.
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

/// Pull `--timeout <duration>` out of `start`'s arguments, leaving the rest for the shared parser.
fn extract_timeout(args: Vec<String>) -> Result<(Vec<String>, Option<Duration>), ExitCode> {
    let mut rest = Vec::with_capacity(args.len());
    let mut timeout = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match value_flag(&arg, &mut it, &["--timeout"]) {
            Some(v) => match prova_core::model::parse_duration(&v) {
                Some(d) => timeout = Some(d),
                None => {
                    eprintln!("prova start: --timeout wants a duration like \"15m\", got {v:?}");
                    return Err(ExitCode::from(2));
                }
            },
            None => rest.push(arg),
        }
    }
    Ok((rest, timeout))
}

/// The `startup` this topology declares, or the default. A manifest that cannot be read at all is
/// not this function's error to report — the spawned `prova up` will say so far better — so it
/// falls back rather than failing here.
fn declared_startup(home: &home::Home, name: &str, manifest_path: Option<&str>) -> Duration {
    const DEFAULT: Duration = Duration::from_secs(300);
    let _ = manifest_path; // `home` already resolved against it
    let Ok(text) = std::fs::read_to_string(&home.manifest) else {
        return DEFAULT;
    };
    let Ok(manifest) = toml::from_str::<crate::manifest::Manifest>(&text) else {
        return DEFAULT;
    };
    manifest
        .topologies
        .get(name)
        .and_then(|d| d.startup.as_deref())
        .and_then(prova_core::model::parse_duration)
        .unwrap_or(DEFAULT)
}

/// Stop the detached holder the way `prova down` does — SIGTERM, so its in-process teardown runs
/// and releases what it created (docs/design/agent-ergonomics.md#start-timeout-orphans-containers).
/// A SIGKILL here is why a timed-out start used to leave its containers behind, and why the NEXT
/// attempt failed on a host port the orphans still held. Escalates only if the holder will not go.
///
/// Owns the *outcome* line for all three endings, so "what happened to my environment" is answered
/// in one place whether the stop came from an expired budget or from Ctrl-C. Keeps relaying while it
/// waits: tearing down a cluster is not instant, and silence during a teardown reads exactly like
/// silence during a startup — as a wedge.
fn stop_holder(child: &mut std::process::Child, name: &str, relay: &mut LogRelay) {
    // A second signal during the grace window means "stop waiting", not "kill it": the holder is
    // ALREADY running its teardown, and killing it there is precisely how containers get stranded.
    // So we step back and say where it went, rather than doing the destructive thing faster.
    let asked_once = interrupt::count();
    runstate::terminate(child.id());
    let grace = Instant::now() + Duration::from_secs(60);
    while Instant::now() < grace {
        relay.pump_remaining();
        if matches!(child.try_wait(), Ok(Some(_))) {
            relay.pump_remaining();
            eprintln!("prova start: {name:?} stopped — what it created was released.");
            return;
        }
        if interrupt::count() > asked_once {
            // Deliberately NOT `prova ps` / `prova down` here: we only reach this wait before the
            // holder registered, so there is no record for either verb to find. The pid and the log
            // are what actually exist, so they are what the line names.
            eprintln!(
                "prova start: {name:?} is still releasing what it created — leaving it to finish in \
                 the background (pid {}); watch {} if it lingers",
                child.id(),
                relay.path.display()
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "prova start: {name:?} did not release within 60s of the stop signal; killing the holder \
         (resources it created may survive — check `docker ps`)"
    );
    let _ = child.kill();
}

/// Streams the detached holder's log to *our* stderr while `start` waits for it to come up.
///
/// `prova up` and `prova start` provision through the identical code path, so the holder is already
/// producing exactly the activity an attached `up` prints — image pulls, builds, readiness waits.
/// It was simply going somewhere nobody was looking: the child's stdio is redirected to
/// `<var>/running/<name>.log` so it can outlive us, and `start` then sat silent for however long the
/// stack took, which for a real cluster is minutes of a cursor. That silence is indistinguishable
/// from a wedge, which is the reason the activity renderer exists at all
/// (docs/plans/run-progress-feedback.md) — and detached mode was the one place its output could not
/// be seen.
///
/// Relaying it back is the whole fix: same lines, same order, no second renderer to keep in step
/// with the first.
///
/// # Where it stops, and why that is a marker rather than a race
///
/// The log carries both of the holder's streams (`runstate::detach` points them at one file), so it
/// ends with the *ready block* — the endpoints `print_endpoints` writes. `start` prints that block
/// itself, on **stdout**, because that is where a caller piping `prova start` has always found the
/// endpoints and moving them to stderr would be a silent break. So the relay must stop exactly
/// where the ready block begins, or the endpoints appear twice.
///
/// It stops on the block's own first line, which both sides of this file agree on — not on "the
/// record showed up, so probably nothing more is coming", which loses whatever the holder wrote in
/// the poll interval before it (in practice the final `— done in 8.3s`, the one line a reader is
/// most likely to be waiting for). Relaying only COMPLETE lines is what makes the marker reliable:
/// a boundary can never fall inside it.
struct LogRelay {
    path: PathBuf,
    /// How far into the log we have already echoed.
    pos: u64,
    /// The first line of `print_endpoints`' block, with its leading blank line — the boundary
    /// between "the holder is working" (ours to echo) and "the holder is up" (`start`'s to print).
    ready_marker: Vec<u8>,
    /// Set once the marker is seen: everything after it belongs to the ready block.
    stopped: bool,
}

impl LogRelay {
    fn new(path: PathBuf, name: &str) -> LogRelay {
        LogRelay {
            path,
            pos: 0,
            ready_marker: format!("\n  {name} — up:").into_bytes(),
            stopped: false,
        }
    }

    /// Every complete line written since the last read, consuming them. A trailing partial line is
    /// left for next time: it keeps `ready_marker` from being split across two reads, and a
    /// half-written progress line is not worth showing early.
    fn take_lines(&mut self) -> Vec<u8> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            // Truncated under us (a re-created log): start over rather than read from nowhere.
            self.pos = 0;
            if f.seek(SeekFrom::Start(0)).is_err() {
                return Vec::new();
            }
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        match buf.iter().rposition(|b| *b == b'\n') {
            Some(i) => buf.truncate(i + 1),
            None => buf.clear(),
        }
        self.pos += buf.len() as u64;
        buf
    }

    /// Echo the holder's new output, stopping for good at the ready block.
    fn pump(&mut self) {
        if self.stopped {
            return;
        }
        let buf = self.take_lines();
        if buf.is_empty() {
            return;
        }
        match find(&buf, &self.ready_marker) {
            Some(cut) => {
                self.write(&buf[..cut]);
                self.stopped = true;
                // Leave the ready block unconsumed: an ending that never reaches "up" still wants
                // it (see `pump_remaining`), and this is the only copy.
                self.pos -= (buf.len() - cut) as u64;
            }
            None => self.write(&buf),
        }
    }

    /// Echo everything left, ready block and partial line included — for the endings where the
    /// topology never came up, so the reason is on screen beside the verdict instead of behind a
    /// `cat` of a path. Ignores the marker deliberately: a holder that printed endpoints and then
    /// failed to register is telling us something, and it is the timeout's most useful clue.
    fn pump_remaining(&mut self) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return;
        };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return;
        }
        self.pos += buf.len() as u64;
        self.write(&buf);
    }

    /// Best-effort by design: a relay that cannot write must never be the reason a topology fails
    /// to start. stderr, because this is the holder's activity — stdout stays the ready block's.
    fn write(&self, bytes: &[u8]) {
        use std::io::Write;
        if bytes.is_empty() {
            return;
        }
        let mut err = std::io::stderr();
        let _ = err.write_all(bytes);
        let _ = err.flush();
    }

    /// Whether anything has been relayed at all — the failure paths fall back to a log tail when
    /// nothing has, so an unreadable log still surfaces the holder's last words.
    fn relayed_anything(&self) -> bool {
        self.pos > 0
    }
}

/// The first index at which `needle` occurs in `haystack`. Small and local: the only search this
/// file needs is one short marker against one poll's worth of lines.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// `prova start <topology>` — stand up a topology **detached**: spawn `prova up <topology>` in its
/// own process group (stdio → a log file), **relay what it says while it comes up**, and once it
/// self-registers print the endpoints and return, leaving it running. `prova down` stops it.
///
/// The two things that make it feel like `up` rather than a black box are both here: the holder's
/// activity is streamed back (`LogRelay`), and a Ctrl-C is caught and turned into the holder's own
/// teardown (`interrupt`) instead of a silent orphan.
pub(crate) fn start_subcommand(args: Vec<String>) -> ExitCode {
    // Detached provisions hold no lease (verifiers.md#detached-topologies-hold-no-lease):
    // everything this invocation spawns is MEANT to outlive it — `prova down` is its reaper.
    prova_core::lease::set_detached();
    // `--timeout` is start's alone (docs/design/agent-ergonomics.md#start-timeout-is-unconfigurable):
    // it bounds the wait for the holder to register, which no other topology verb does. Extracted
    // here rather than in the shared flag parser so `prova down --timeout` still refuses it instead
    // of silently accepting a flag it would ignore.
    let (args, cli_timeout) = match extract_timeout(args) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (name, manifest_path, profile, fixed) = match parse_topology_args("start", args) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let home = match resolve_home(manifest_path.as_deref()) {
        Ok(h) => h,
        Err(code) => return code,
    };

    if let Err(code) = guard_double_provision("start", &home, &name) {
        return code;
    }
    if let Err(e) = runstate::dir(&home) {
        eprintln!("prova start: cannot create run-state dir: {e}");
        return ExitCode::from(2);
    }

    let exe = match prova_core::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("prova start: cannot find the prova executable: {e}");
            return ExitCode::from(2);
        }
    };
    // Arm BEFORE the spawn: the window between spawning a holder and beginning to watch it is
    // precisely the window in which an interrupt would orphan one. `exec` resets handled signals to
    // their default in the child, so the holder still dies of a signal it is sent directly — which
    // is what `stop_holder` and `prova down` rely on.
    interrupt::arm();

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

    // Ours to say, and said immediately: the holder's own first line can be seconds away (manifest
    // resolution, a git package fetch), and dead air before it is the same silence this verb is
    // being taught not to produce. Naming the log makes the stream that follows attributable, and
    // leaves the reader something to `tail -f` after we exit.
    eprintln!(
        "prova: starting topology {name:?} detached — following {}",
        log.display()
    );
    // How long the holder may take to register is the TOPOLOGY's fact, not prova's: a kind cluster
    // with eight rollouts is honestly minutes, and a fixed window made the inhabited half of the
    // inhabited/fixture pair unavailable to it
    // (docs/design/agent-ergonomics.md#start-timeout-is-unconfigurable). Flag, then declaration,
    // then the default.
    let budget =
        cli_timeout.unwrap_or_else(|| declared_startup(&home, &name, manifest_path.as_deref()));
    await_registration(&mut child, &home, &name, budget)
}

/// Drop the run-state record — but only if the holder is actually gone.
///
/// The holder removes its own record on the way out, so this is the belt-and-braces for when it
/// cannot. Conditioning it on the child being dead matters now that the record appears at birth: a
/// holder we stopped but which is STILL RELEASING owns its record until it exits, and a holder that
/// never exits leaves a starting-record behind on purpose — that record is the evidence a factory
/// got half-way (docs/design/agent-ergonomics.md#a-stale-starting-record-implies-residue). Deleting
/// it here would erase the one signal that tells the next attempt its collision is residue.
fn forget_if_gone(child: &mut std::process::Child, home: &home::Home, name: &str) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        runstate::remove(home, name);
    }
}

/// Wait for the detached holder to self-register (up), exit (failed), be interrupted, or exhaust
/// its budget — relaying what it says while it works.
///
/// Split out so `start_subcommand` stays within the function-length ratchet, and because the wait
/// is a coherent thing on its own: it owns the budget, the four verdicts, and the graceful stop.
fn await_registration(
    child: &mut std::process::Child,
    home: &home::Home,
    name: &str,
    budget: Duration,
) -> ExitCode {
    let deadline = Instant::now() + budget;
    let mut relay = LogRelay::new(runstate::log_path(home, name), name);
    loop {
        // Relay first, then decide. The relay stops itself at the ready block, so this is safe in
        // either order — but doing it first means the last thing the holder said before coming up
        // is on screen before the endpoints that answer it.
        relay.pump();
        // READY, not merely present. The record now appears at birth so the topology is visible
        // and guarded while it provisions — which means "a record exists" stopped being the
        // came-up signal it used to be. Waiting on presence alone would make `start` return the
        // instant the holder registered its intent: exit 0, an empty endpoint block, and a
        // detached process still creating a cluster nobody was told about.
        if let Some(rec) = runstate::read(home, name).filter(|r| r.status == runstate::Status::Ready)
        {
            let eps: Vec<prova_core::Endpoint> = rec
                .endpoints
                .iter()
                .map(|e| prova_core::Endpoint {
                    name: e.name.clone(),
                    url: e.url.clone(),
                })
                .collect();
            print_endpoints(name, &eps);
            println!(
                "\n  started (pid {}) — `prova down {name}` to stop, `prova ps` to list",
                rec.pid
            );
            return ExitCode::SUCCESS;
        }

        // The interrupt, before the budget and before the child check: the user asked to stop, and
        // a half-provisioned environment is exactly what must not survive that. This is the same
        // teardown Ctrl-C gets under an attached `up` — the difference was only ever that our child
        // lives in its own process group and never hears the terminal.
        if interrupt::raised() {
            eprintln!("\nprova start: interrupted — stopping topology {name:?}…");
            stop_holder(child, name, &mut relay);
            forget_if_gone(child, home, name);
            return ExitCode::from(130);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                relay.pump_remaining();
                eprintln!(
                    "prova start: topology {name:?} failed to come up (child exited: {status})"
                );
                // The relay has normally already shown the holder's last words in order. The tail
                // is the fallback for when it could not read the log at all — a failure whose
                // diagnosis is on disk and not on screen is the one thing worse than the failure.
                if !relay.relayed_anything() {
                    let tail = runstate::log_tail(home, name, 20);
                    if !tail.trim().is_empty() {
                        eprintln!("--- {name} log (tail) ---\n{tail}");
                    }
                }
                runstate::remove(home, name);
                return ExitCode::from(2);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("prova start: lost track of the child process: {e}");
                return ExitCode::from(2);
            }
        }
        if Instant::now() >= deadline {
            relay.pump_remaining();
            eprintln!(
                "prova start: topology {name:?} did not come up within {budget:?} — stopping it. \
                 Declare what it needs (`startup = \"15m\"` on the [topologies] entry — the \
                 definition knows its own cost), or override this invocation with --timeout."
            );
            stop_holder(child, name, &mut relay);
            forget_if_gone(child, home, name);
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
        // A live holder reports its own state; a dead one is stale whatever it claimed to be.
        // "up 90s" for something still creating a cluster was the lie worth removing: the elapsed
        // time is real, what it measures is not the same thing in both states.
        let status = if alive { rec.status.label() } else { "stale" };
        let elapsed = now.saturating_sub(rec.started_at);
        let verb = match (alive, rec.status) {
            (true, runstate::Status::Starting) => "starting for",
            _ => "up",
        };
        println!(
            "{}  [{}]  pid {}  {} {}s",
            rec.name, status, rec.pid, verb, elapsed
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

/// `--fresh` over a LIVE holder: this run provisions its own instance of a name something else is
/// already holding (docs/design/topologies.md#fresh-over-a-holder-is-announced). Harmless when the
/// definition names its resources per-instance — two random-port stacks coexist — and destructive
/// when it does not: a fixed-name cluster (`kind create --name ybor-studio`) collides on creation,
/// and this run's teardown then reaps the HOLDER's cluster, because both spell the same name.
///
/// A warning rather than a refusal: prova cannot see from here whether the definition uses fixed
/// names, and `--fresh` beside a holder is legitimate for every definition that does not. The
/// warning names both exits so the destructive case is a choice rather than a surprise.
pub(crate) fn warn_fresh_over_a_holder(home: &Option<Home>) {
    let Some(h) = home else { return };
    for rec in runstate::list(h) {
        if !runstate::is_alive(rec.pid) {
            continue;
        }
        // A STARTING holder is the sharper edge of the same warning: it is actively creating the
        // very names this run is about to create, so the collision is not hypothetical timing — it
        // is two factories in the same seconds.
        let state = match rec.status {
            runstate::Status::Starting => "still STARTING",
            runstate::Status::Ready => "held",
        };
        eprintln!(
            "prova: --fresh with topology {:?} {state} (pid {}): this run provisions its OWN instance \
             — if the definition uses fixed names or fixed host ports, the two collide and THIS \
             run's teardown reaps the holder's resources. `prova down {}` first, or drop --fresh to \
             attach.",
            rec.name, rec.pid, rec.name
        );
    }
}

/// Run-wide topologies (docs/design/topologies.md#run-wide-topology-is-provisioned-once): each
/// `[topologies]` entry declaring `scope = "run"` is provisioned ONCE for this run, by a pool whose
/// holder outlives every suite, and every declaring file binds that instance instead of building
/// its own. Returns the pool so the caller can reap it when the run ends — dropping it reaps too,
/// so no exit path can leak the environment.
///
/// Nothing is provisioned here: the pool is demand-driven, so a `-k` run that reaches no test using
/// the topology pays nothing. That is what makes declaring an expensive environment run-wide safe.
pub(crate) fn intern_run_wide_topologies(
    env: &dispatch::RunEnv,
    config: &mut prova_core::RunConfig,
) -> Result<Option<prova_core::TopologyPool>, ExitCode> {
    let mut names: Vec<String> = Vec::new();
    // The holder's config is this run's, plus the registrations it must be able to rebuild: a
    // fresh Lua state can only reach a factory through `require`, so the REGISTRATION is the
    // definition a run-wide instance is built from (a proof file's declaration of the same name is
    // the demand, exactly as under attach).
    let mut holder = config.clone();
    for (alias, decl) in &env.env.topologies {
        // Every entry is validated, whether or not this run would use it: a refusal that depends
        // on the selection is not a refusal.
        match decl.sharing(alias) {
            Ok(manifest::TopologyScope::File) => continue,
            Ok(manifest::TopologyScope::Run) => {}
            Err(e) => {
                eprintln!("prova: {e}");
                return Err(ExitCode::from(2));
            }
        }
        let resolved = match packages::resolve_topology(alias, decl, &env.env.dependencies) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("prova: {e}");
                return Err(ExitCode::from(2));
            }
        };
        holder = holder.with_topology_registration(
            alias,
            &decl.package,
            resolved.factory,
            topology_options_to_lua(&decl.options),
        );
        names.push(alias.clone());
    }
    if names.is_empty() {
        return Ok(None);
    }
    let pool = prova_core::TopologyPool::start(names, &holder);
    let installed = std::mem::take(config).with_interned_topologies(pool.handle());
    *config = installed;
    Ok(Some(pool))
}

/// Reap whatever the run-wide pool provisioned, and say so: tearing down a cluster takes long
/// enough that silence reads as a hang, and a leaked one must never read as a clean exit.
pub(crate) fn reap_run_wide_topologies(pool: Option<prova_core::TopologyPool>) {
    let Some(mut pool) = pool else { return };
    let held = pool.provisioned();
    if !held.is_empty() {
        eprintln!(
            "prova: tearing down run-wide topolog{} {}",
            if held.len() == 1 { "y" } else { "ies" },
            held.join(", ")
        );
    }
    pool.shutdown();
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

        assert!(topology_flags("down", "usage", 1, args(&["a", "b"])).is_err(), "over the cap");
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
