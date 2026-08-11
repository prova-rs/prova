//! The dispatcher: `run(cli_args)` — flag parsing and the verb switch, decomposed along the
//! pipeline's own phases: parse flags → resolve home + environment → assemble config → answer
//! query verbs → run and conclude. `Err(ExitCode)` throughout means "exit the process with this
//! code now" — including the successful early exits (`--version`, `--help`, the listings).

use super::*;

/// Everything the flag loop can set — one struct instead of thirty locals.
struct Cli {
    format: Option<Format>,
    color: Option<report::ColorMode>,
    progress: Option<progress::Mode>,
    quiet: bool,
    heed: crate::manifest::Heed,
    switches: Vec<String>,
    junit: Option<String>,
    gha: Option<report::GhaMode>,
    jobs: Option<usize>,
    update_snapshots: bool,
    update_baseline: bool,
    unreferenced: String, // ignore | warn | delete
    config: Option<String>,
    list: bool,
    // Internal: `prova tests` routes here as `--list --list-tagged` so the listing renders each node
    // with its promise⇄proof state (PROMISE / PROOF), while plain `--list` (and the retiring
    // `prova list`) stays bare paths. Not a user-facing flag.
    list_tagged: bool,
    // Internal: `prova specs backfill` routes here to list the proofs no claim backs (empty `covers`)
    // — the reverse of `owed`, a red→green worklist that gates (exit ≠0 while any is unbacked). Not a
    // user-facing flag.
    backfill: bool,
    // Internal: `prova reminders` routes here to COLLECT declared reminders (loading the suite,
    // like `--list`, without executing) and overlay the recorded state — so the verb works before
    // any run and shows live states after one. Not a user-facing flag.
    reminders_list: bool,
    reminders_state: Option<&'static str>,
    switches_list: bool,
    promises_only: bool,
    proofs_only: bool,
    falsify: bool,
    // Held-topology attach (docs/design/topologies.md#attach-binds-by-name): `--fresh` opts a run
    // out of attaching to held topologies; `--topology NAME` insists on attaching to NAME.
    fresh: bool,
    require_topology: Option<String>,
    due: bool,
    explicit_paths: Vec<String>,
    profile: Option<String>,
    manifest_path: Option<String>,
    packages: Vec<String>,
    selection: prova_core::Selection,
    last_failed: bool,
    record_to: Option<std::path::PathBuf>,
    // `--allow-empty`: opt out of the empty-selection error, for the matrix leg that legitimately
    // selects nothing. Off by default, because a selection matching nothing is nearly always a typo
    // and a typo must not be green.
    allow_empty: bool,
    // Git-source freshness overrides for this run. `-U`/`--update` forces plugin updates (skips the
    // TTL + remote-hash gates); `--offline` forbids any network, using only what's already cached.
    update_force: bool,
    offline: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            format: None,
            color: None,
            progress: None,
            quiet: false,
            heed: crate::manifest::Heed::None,
            switches: Vec::new(),
            junit: None,
            gha: None,
            jobs: None,
            update_snapshots: false,
            update_baseline: false,
            unreferenced: String::from("ignore"),
            config: None,
            list: false,
            list_tagged: false,
            backfill: false,
            reminders_list: false,
            reminders_state: None,
            switches_list: false,
            promises_only: false,
            proofs_only: false,
            falsify: false,
            fresh: false,
            require_topology: None,
            due: false,
            explicit_paths: Vec::new(),
            profile: None,
            manifest_path: None,
            packages: Vec::new(),
            selection: prova_core::Selection::default(),
            last_failed: false,
            record_to: None,
            allow_empty: false,
            update_force: false,
            offline: false,
        }
    }
}

fn parse_cli(cli_args: Vec<String>) -> Result<Cli, ExitCode> {
    let mut cli = Cli::default();
    let mut args = cli_args.into_iter();
    while let Some(arg) = args.next() {
        if cli.run_flag(&arg, &mut args)? || cli.output_flag(&arg, &mut args)? {
            continue;
        }
        cli.bare_flag(&arg)?;
    }
    Ok(cli)
}

impl Cli {
    /// The value flags that shape WHAT runs: selection axes, profile/manifest, packages, jobs,
    /// topology attach, the record path, and selective heed. Returns whether `arg` was consumed.
    fn run_flag(
        &mut self,
        arg: &str,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<bool, ExitCode> {
        // `-P name=source` (repeatable): an ad-hoc package, layered over the manifest (CLI wins).
        if let Some(v) = value_flag(arg, args, &["--package", "-P", "--plugin"]) {
            if arg.starts_with("--plugin") {
                eprintln!("prova: `--plugin` is deprecated — use `--package` (retires at 1.0)");
            }
            self.packages.push(v);
        // `-k pattern` (repeatable): case-insensitive substring of the node path; `!pat` excludes.
        } else if let Some(v) = value_flag(arg, args, &["-k"]) {
            match v.strip_prefix('!') {
                Some(rest) => self.selection.keyword_excludes.push(rest.to_string()),
                None => self.selection.keywords.push(v),
            }
        // `--tags a,b` (repeatable): leaf has any listed tag; `!tag` excludes.
        } else if let Some(v) = value_flag(arg, args, &["--tags"]) {
            for t in v.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                match t.strip_prefix('!') {
                    Some(rest) => self.selection.tag_excludes.push(rest.to_string()),
                    None => self.selection.tags.push(t.to_string()),
                }
            }
        // `--node "full › node › path"` (repeatable): exact node selection — re-run precisely the
        // node a report named.
        } else if let Some(v) = value_flag(arg, args, &["--node"]) {
            self.selection.nodes.push(v);
        // `-s class` / `--switch a,b` (repeatable): throw opt-in switches — authorize the named
        // classes for this run (docs/design/manifest.md#switches-not-env-capabilities). Unions
        // with `[run]`/profile `switches`; a throw authorizes, it never widens a profile's scope.
        } else if let Some(v) = value_flag(arg, args, &["--switch", "-s"]) {
            for s in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                self.switches.push(s.to_string());
            }
        } else if let Some(v) = value_flag(arg, args, &["--jobs", "-j"]) {
            match v.parse::<usize>() {
                Ok(n) if n >= 1 => self.jobs = Some(n),
                _ => {
                    eprintln!("prova: --jobs expects a positive integer, got {v:?}");
                    return Err(ExitCode::from(2));
                }
            }
        } else if let Some(v) = value_flag(arg, args, &["--profile", "-p"]) {
            self.profile = Some(v);
        } else if let Some(v) = value_flag(arg, args, &["--manifest"]) {
            self.manifest_path = Some(v);
        // `--topology NAME`: REQUIRE attaching to the held topology NAME — error rather than
        // silently provisioning fresh, because a run meant to judge a live environment (e.g. a
        // Tilt-injected work-in-progress build) must never quietly test something else.
        } else if let Some(v) = value_flag(arg, args, &["--topology"]) {
            self.require_topology = Some(v);
        // `--record <path>`: ALSO emit the run record here. The var/ copy is written either way and
        // is for the next command; this one is for CI to keep as an artifact, or a human to read.
        } else if let Some(v) = value_flag(arg, args, &["--record"]) {
            self.record_to = Some(std::path::PathBuf::from(v));
        // `--heed` (bare) heeds all DUE reminders; `--heed=<sel>[,<sel>]` heeds only the matching
        // ones (by reminder name/tag) — the ad-hoc form of a profile's `heed` list. Accumulates.
        } else if let Some(sels) = arg.strip_prefix("--heed=") {
            let list: Vec<String> = sels
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            self.heed =
                std::mem::take(&mut self.heed).merge(crate::manifest::Heed::Matching(list));
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// The value flags that shape OUTPUT: format, progress, color, report sinks, snapshot
    /// reconciliation, and the companion config override. Returns whether `arg` was consumed.
    fn output_flag(
        &mut self,
        arg: &str,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<bool, ExitCode> {
        // `--format json` and `--format=json` both work.
        if let Some(v) = value_flag(arg, args, &["--format"]) {
            match v.as_str() {
                "json" => self.format = Some(Format::Json),
                "console" => self.format = Some(Format::Console),
                "tap" => self.format = Some(Format::Tap),
                other => {
                    eprintln!("prova: unknown format {other:?} (expected console|json|tap)");
                    return Err(ExitCode::from(2));
                }
            }
        // `--progress auto|always|never`: report what a long pause IS (pulling, waiting) on
        // stderr. Separate from `--color` because it is a different stream and a different concern:
        // color styles stdout, progress narrates stderr, and neither can corrupt a machine format.
        } else if let Some(v) = value_flag(arg, args, &["--progress"]) {
            match progress::Mode::parse(&v) {
                Ok(mode) => self.progress = Some(mode),
                Err(e) => {
                    eprintln!("prova: {e}");
                    return Err(ExitCode::from(2));
                }
            }
        // `--color auto|always|never`: color the console output (auto = only on a terminal).
        } else if let Some(v) = value_flag(arg, args, &["--color"]) {
            match report::ColorMode::parse(&v) {
                Some(mode) => self.color = Some(mode),
                None => {
                    eprintln!("prova: unknown --color {v:?} (expected auto|always|never)");
                    return Err(ExitCode::from(2));
                }
            }
        // `--junit PATH`: write a JUnit XML report to a file, alongside whatever --format prints.
        } else if let Some(v) = value_flag(arg, args, &["--junit"]) {
            self.junit = Some(v);
        // `--gha auto|on|off`: the GitHub Actions sink (annotations + step summary).
        } else if let Some(v) = value_flag(arg, args, &["--gha"]) {
            match report::GhaMode::parse(&v) {
                Some(mode) => self.gha = Some(mode),
                None => {
                    eprintln!("prova: unknown --gha {v:?} (expected auto|on|off)");
                    return Err(ExitCode::from(2));
                }
            }
        // `--unreferenced ignore|warn|delete`: what to do with `.snap` files no test referenced.
        } else if let Some(v) = value_flag(arg, args, &["--unreferenced"]) {
            match v.as_str() {
                "ignore" | "warn" | "delete" => self.unreferenced = v,
                other => {
                    eprintln!(
                        "prova: unknown --unreferenced {other:?} (expected ignore|warn|delete)"
                    );
                    return Err(ExitCode::from(2));
                }
            }
        // `--config PATH`: override the companion config file (else `PROVA_CONFIG`, else the manifest
        // `config`, else `prova.lua`). Chiefly a testing affordance — point a run at a specific
        // config without editing a manifest.
        } else if let Some(v) = value_flag(arg, args, &["--config"]) {
            self.config = Some(v);
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// The bare flags and positional paths. `--version`/`--help` print and exit successfully.
    fn bare_flag(&mut self, arg: &str) -> Result<(), ExitCode> {
        match arg {
            "--list" => self.list = true,
            "--list-tagged" => {
                self.list = true;
                self.list_tagged = true;
            }
            "--backfill" => self.backfill = true,
            "--reminders-list" => self.reminders_list = true,
            "--switches-list" => self.switches_list = true,
            // Internal spellings of the reminders lane's state filters (`prova reminders --due`);
            // rewritten by reminders_subcommand because bare `--due` is the promise decree below.
            "--reminders-due" => self.reminders_state = Some("due"),
            "--reminders-watching" => self.reminders_state = Some("watching"),
            "--quiet" | "-q" => self.quiet = true,
            // Promote this one invocation to heed the attention account — the ad-hoc form of the
            // manifest's `heed`; like every guarantee it can only tighten (All absorbs).
            "--heed" => self.heed = crate::manifest::Heed::All,
            "--last-failed" => self.last_failed = true,
            "--falsify" => self.falsify = true,
            "--fresh" => self.fresh = true,
            "--promises" => self.promises_only = true,
            "--proofs" => self.proofs_only = true,
            "--due" => self.due = true,
            "--allow-empty" => self.allow_empty = true,
            "--update-snapshots" | "-u" => self.update_snapshots = true,
            "--update-baseline" => self.update_baseline = true,
            "--update" | "-U" => self.update_force = true,
            "--offline" => self.offline = true,
            "--json" => self.format = Some(Format::Json),
            "--version" | "-V" => {
                println!("prova {}", prova_core::VERSION);
                return Err(ExitCode::SUCCESS);
            }
            "--help" | "-h" => {
                println!("{}", help_text());
                return Err(ExitCode::SUCCESS);
            }
            other if other.starts_with('-') => {
                eprintln!("prova: unknown flag {other}");
                return Err(ExitCode::from(2));
            }
            other => self.explicit_paths.push(other.to_string()),
        }
        Ok(())
    }
}

/// Determine the prova home (the directory owning `prova.toml`). `--manifest PATH` points
/// directly at a manifest; a manifest-mode run walks up from the current directory; explicit
/// path args anchor discovery at the NAMED paths themselves — a file selects what to run but
/// keeps its package's environment, even when named from outside the package. An ambiguous
/// layout (more than one manifest location) is an error, as is a selection spanning packages.
fn resolve_run_home(cli: &Cli) -> Result<Option<Home>, ExitCode> {
    if let Some(path) = &cli.manifest_path {
        return Ok(Some(home::from_manifest_path(Path::new(path))));
    }
    let found = if !cli.explicit_paths.is_empty() {
        home_for_explicit_paths(&cli.explicit_paths)
    } else {
        home::find(Path::new(".")).map_err(|e| e.to_string())
    };
    found.map_err(|e| {
        eprintln!("prova: {e}");
        ExitCode::from(2)
    })
}

/// The resolved environment a run executes in: where proofs anchor, what to run, and the
/// manifest's answers (or ad-hoc defaults when no package owns the named paths).
struct RunEnv {
    base_dir: PathBuf,
    paths: Vec<String>,
    env: ManifestRun,
}

/// Resolve the run. Explicit path args are the SELECTION (literal paths relative to cwd, no
/// declared-suite fan-out, no IDE management) but never strip the package environment: when the
/// named paths live in a package, its manifest still supplies plugins, capabilities, and run
/// defaults. Otherwise read the home's `prova.toml` (a `proofs` name-pattern rooted at home).
fn resolve_env(cli: &mut Cli, home: &Option<Home>, layout: &XdgSystemLayout) -> Result<RunEnv, ExitCode> {
    let explicit = !cli.explicit_paths.is_empty();
    if !explicit {
        let Some(home) = &home else {
            eprintln!(
                "usage: prova <file-or-dir>...   or   prova [--profile NAME]  (reads prova.toml)"
            );
            return Err(ExitCode::from(2));
        };
        let mut env = resolve_from_manifest(
            home,
            cli.profile.clone(),
            cli.jobs,
            cli.format,
            cli.config.take(),
            layout,
            cli.update_force,
            cli.offline,
            true,
        )?;
        return Ok(RunEnv {
            // `home.dir` IS the package root (the parent of a nested `.prova/`/`prova/` nook), so
            // proof patterns, `config`, and `plugin_root` all resolve against it. `proofs/` lives
            // at the root while prova's own files tuck into the nook.
            base_dir: home.dir.clone(),
            paths: std::mem::take(&mut env.proofs),
            env,
        });
    }
    let paths = std::mem::take(&mut cli.explicit_paths);
    match &home {
        // The named paths belong to a package: borrow its environment (plugins, capabilities,
        // jobs/format defaults — CLI flags still win inside resolve_from_manifest), while the
        // selection stays the explicit paths and `[suites.*]` declarations do not fan out.
        Some(home) => {
            let mut env = resolve_from_manifest(
                home,
                cli.profile.clone(),
                cli.jobs,
                cli.format,
                cli.config.take(),
                layout,
                cli.update_force,
                cli.offline,
                false,
            )?;
            env.suites = BTreeMap::new();
            env.manage = Manage::Never;
            Ok(RunEnv { base_dir: PathBuf::from("."), paths, env })
        }
        // No package anywhere above the named paths: an ad-hoc run with built-ins only.
        None => Ok(RunEnv {
            base_dir: PathBuf::from("."),
            paths,
            env: ManifestRun {
                proofs: Vec::new(),
                jobs: cli.jobs.unwrap_or(1),
                format: cli.format.unwrap_or(Format::Console),
                color: None,
                progress: None,
                quiet: None,
                github: None,
                junit: None,
                suites: BTreeMap::new(),
                dependencies: packages::ResolvedPackages::default(),
                sources: BTreeMap::new(),
                manage: Manage::Never,
                topologies: BTreeMap::new(),
                // No manifest, so there is no companion — built-in capabilities still work;
                // registered ones are simply absent.
                capabilities: prova_core::Capabilities::default(),
                globals_inject: Vec::new(),
                // No manifest, no [placement]; the env var is still honoured at the handshake.
                placement_broker: None,
                heed: crate::manifest::Heed::None, // no manifest, nothing promised attention
                lane_tags: Vec::new(),             // no manifest, no lanes
                switches: Vec::new(),              // no manifest, nothing thrown
            },
        }),
    }
}

/// Progress resolution mirrors color's ladder: CLI flag > `PROVA_PROGRESS` > manifest > auto.
/// A bad env value is a hard error rather than a silent fallback — a typo'd `PROVA_PROGRESS=nevr`
/// that quietly leaves activity ON is the sort of thing someone debugs for ten minutes.
fn resolve_progress(cli: &Cli, manifest: &ManifestRun) -> Result<progress::Mode, ExitCode> {
    let mode = match cli.progress {
        Some(m) => m,
        None => match std::env::var("PROVA_PROGRESS") {
            Ok(v) if !v.trim().is_empty() => match progress::Mode::parse(&v) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("prova: {e} (from PROVA_PROGRESS)");
                    return Err(ExitCode::from(2));
                }
            },
            _ => manifest.progress.unwrap_or(progress::Mode::Auto),
        },
    };
    // `--quiet` silences the reporter's stdout chatter; it silences activity too, because someone
    // asking for quiet means it. `--progress always` is the way back if you want narration without
    // the tree.
    let quiet = cli.quiet || manifest.quiet.unwrap_or(false);
    Ok(if quiet && cli.progress.is_none() { progress::Mode::Never } else { mode })
}

/// Assemble the engine `RunConfig`: builder flags, thrown switches, the selection (CLI axes,
/// `--last-failed` folds, the lane's baked tags), and the shared tracking registries.
#[allow(clippy::too_many_arguments)]
fn build_config(
    cli: &mut Cli,
    env: &mut RunEnv,
    home: &Option<Home>,
    progress_sink: &std::sync::Arc<dyn prova_core::Progress>,
    deputed: &prova_core::DeputedRegistry,
    measurements: &prova_core::MeasurementRegistry,
) -> Result<prova_core::RunConfig, ExitCode> {
    if cli.promises_only && cli.proofs_only {
        eprintln!(
            "prova: --promises and --proofs are mutually exclusive — a test is a promise or a proof, \
             not both"
        );
        return Err(ExitCode::from(2));
    }
    let mut config = engine_config(
        env.env.jobs,
        &env.env.dependencies,
        home.as_ref(),
        std::sync::Arc::clone(progress_sink),
    )
    .with_update_snapshots(cli.update_snapshots)
    .with_due(cli.due)
    .with_promises_only(cli.promises_only)
    .with_proofs_only(cli.proofs_only)
    .with_falsify(cli.falsify)
    // Thrown switches: the manifest's ([run] ∪ profile) ∪ the CLI's `-s` — all doors union
    // (docs/design/manifest.md#switches-not-env-capabilities).
    .with_switches(env.env.switches.iter().cloned().chain(cli.switches.iter().cloned()))
    .with_capabilities(std::mem::take(&mut env.env.capabilities))
    .with_globals_inject(std::mem::take(&mut env.env.globals_inject))
    .with_deputed_tracking(deputed.clone())
    .with_measurement_tracking(measurements.clone());

    // `--last-failed`: fold the previous run's failed node paths into the selection as exact nodes.
    if cli.last_failed {
        match load_last_failed(home) {
            Some(paths) if !paths.is_empty() => cli.selection.nodes.extend(paths),
            _ => eprintln!(
                "prova: --last-failed: no failure state from a previous run here; running everything"
            ),
        }
    }
    // The lane's baked tags join the selection as their own gate (`!` splits into excludes,
    // same grammar as --tags). The CLI's axes then narrow WITHIN the lane.
    for t in &env.env.lane_tags {
        match t.strip_prefix('!') {
            Some(rest) => cli.selection.lane_tag_excludes.push(rest.to_string()),
            None => cli.selection.lane_tags.push(t.clone()),
        }
    }
    config.selection = std::mem::take(&mut cli.selection);
    Ok(config)
}

/// Held-topology attach (docs/design/topologies.md#attach-binds-by-name): unless `--fresh`,
/// every LIVE held record is offered to the run; the engine binds the ones the collection
/// actually declares, by name, instead of provisioning — and reports each into the returned
/// registry so the run record can carry live-state provenance. Announced up front: an attached
/// run is deliberately non-hermetic, and that must never be silent.
fn attach_held(
    cli: &Cli,
    home: &Option<Home>,
    mut config: prova_core::RunConfig,
) -> Result<(prova_core::RunConfig, prova_core::AttachedRegistry), ExitCode> {
    let attached: prova_core::AttachedRegistry = std::sync::Arc::default();
    if cli.fresh {
        if cli.require_topology.is_some() {
            eprintln!("prova: --topology and --fresh contradict each other — pick one");
            return Err(ExitCode::from(2));
        }
        return Ok((config, attached));
    }
    if let Some(h) = &home {
        for rec in runstate::list(h) {
            if !runstate::is_alive(rec.pid) {
                continue;
            }
            if let Some(want) = &cli.require_topology {
                if want != &rec.name {
                    continue;
                }
            }
            eprintln!(
                "prova: held topology {:?} is running (pid {}) — runs that declare it attach to its LIVE state (--fresh to opt out)",
                rec.name, rec.pid
            );
            config = config.with_attached_topology(rec.name.clone(), rec.value.clone());
        }
    }
    if let Some(want) = &cli.require_topology {
        let offered = home
            .as_ref()
            .map(|h| {
                runstate::list(h)
                    .iter()
                    .any(|r| &r.name == want && runstate::is_alive(r.pid))
            })
            .unwrap_or(false);
        if !offered {
            eprintln!(
                "prova: --topology {want:?}: no held topology by that name is running (see `prova ps`; hold one with `prova start {want}`)"
            );
            return Err(ExitCode::from(2));
        }
    }
    config = config.with_attached_tracking(attached.clone());
    Ok((config, attached))
}

/// The opt-in classes, listed: the census from collection (bodies never execute), the
/// thrown-by column from the manifest — so an ad-hoc-only class is a stated fact, never a
/// hidden test population (docs/design/manifest.md#switches-are-discoverable).
fn switches_listing(
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    home: &Option<Home>,
) -> ExitCode {
    let census = prova_core::collect_switch_census(suites, config);
    if census.is_empty() {
        println!(
            "prova: no switches declared — mark an opt-in class with `switch = \"<class>\"` \
             on a test, group, or suite.config (`prova learn running`)"
        );
        return ExitCode::SUCCESS;
    }
    // Who throws each class, straight from the manifest (config already unioned them for THIS
    // run; the listing answers for every profile, so it re-reads the declarations).
    let mut throwers: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    let manifest_text = home
        .as_ref()
        .and_then(|h| std::fs::read_to_string(&h.manifest).ok());
    let parsed = manifest_text.as_deref().and_then(|t| Manifest::parse(t).ok());
    if let Some(m) = &parsed {
        for s in &m.run.switches {
            throwers.entry(s.as_str()).or_default().push("[run] — every run".to_string());
        }
        for (name, profile) in &m.profiles {
            for s in &profile.switches {
                throwers.entry(s.as_str()).or_default().push(format!("profile `{name}`"));
            }
        }
    }
    for (class, gated) in &census {
        let who = match throwers.get(class.as_str()) {
            Some(list) => list.join(", "),
            None => format!("nobody — ad-hoc only (`-s {class}`)"),
        };
        println!("  {class:<12} {gated} gated · thrown by: {who}");
    }
    // The reverse direction: a config throw naming a class nobody declares is a stale row.
    for (class, who) in &throwers {
        if !census.contains_key(*class) {
            println!(
                "  {class:<12} 0 gated · thrown by: {} — no test declares this class (stale?)",
                who.join(", ")
            );
        }
    }
    println!();
    println!(
        "  {} class(es) · throw ad hoc with -s <switch>, or `prova run <profile>` where listed",
        census.len()
    );
    ExitCode::SUCCESS
}

/// `prova specs backfill` — the reverse of `owed`: every proof no claim backs (empty
/// `covers`). A red→green worklist that GATES — exit non-zero while any proof is unbacked —
/// so an agent can drive coverage to complete. It never writes a spec: it names the proof and
/// the agent infers the claim (an auto-stubbed `<!-- claim -->` would be vacuous prose).
fn backfill_listing(suites: &[prova_core::Suite], config: &prova_core::RunConfig) -> ExitCode {
    let mut unbacked: Vec<String> = Vec::new();
    for suite in suites {
        match discover_suite(suite, config) {
            Ok(nodes) => unbacked.extend(nodes.into_iter().filter(|n| !n.backed).map(|n| n.path)),
            Err(err) => {
                eprintln!("prova: {}: {err}", suite.name);
                return ExitCode::from(2);
            }
        }
    }
    if unbacked.is_empty() {
        println!("prova: every proof is backed by a claim — nothing to backfill");
        return ExitCode::SUCCESS;
    }
    println!("proofs no claim backs — write a `<!-- claim: id -->` in a [specs] doc, then add");
    println!("`covers = \"doc.md#id\"` to the proof (`prova learn claims`):");
    println!();
    for path in &unbacked {
        println!("  UNBACKED  {path}");
    }
    println!();
    println!("  {} unbacked", unbacked.len());
    ExitCode::from(1)
}

/// `--list` / `prova tests`: per-suite, never per-file — the setup (`suite.lua`) must load first
/// so the listed collection is exactly what a run would collect (suite-level `spec`/`requires`/
/// name). `--list-tagged` state-tags each node with its side of the promise⇄proof duality;
/// plain `--list` stays bare paths (stable, machine-friendly).
fn nodes_listing(
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    tagged: bool,
) -> ExitCode {
    for suite in suites {
        match discover_suite(suite, config) {
            Ok(nodes) if tagged => nodes.iter().for_each(|n| {
                let tag = if n.promised { "PROMISE" } else { "PROOF" };
                println!("  {tag:<8} {}", n.path);
            }),
            Ok(nodes) => nodes.iter().for_each(|n| println!("{}", n.path)),
            Err(err) => {
                eprintln!("prova: {}: {err}", suite.name);
                return ExitCode::from(2);
            }
        }
    }
    ExitCode::SUCCESS
}

/// Placement (docs/design/placement.md §Transport): if a broker is configured — env var over
/// manifest — dial it and say hello BEFORE anything runs. Configured-but-unreachable is a loud
/// error, never a silent fall back to local: falling back would turn a broken pool into a
/// suite that quietly stopped distributing, and the only symptom would be that it got slower.
/// (Answering `requires`/`resources` from the pool waits on the dispatch planes — see
/// docs/plans/placement-client.md — so today the handshake validates the configuration and
/// announces the pool; the run itself proceeds as before.)
fn placement_handshake(manifest_broker: Option<&str>) -> Result<(), ExitCode> {
    #[cfg(unix)]
    if let Some((addr, source)) = placement::configured(manifest_broker) {
        match placement::hello(&addr) {
            Ok(info) => {
                let plural = if info.nodes == 1 { "" } else { "s" };
                eprintln!(
                    "prova: placement broker {} at {addr} ({} node{plural}, protocol {})",
                    info.broker, info.nodes, info.protocol
                );
            }
            Err(e) => {
                eprintln!("prova: {e} (configured via {source}; fix the address or start the broker — prova never silently falls back to local)");
                return Err(ExitCode::from(2));
            }
        }
    }
    #[cfg(not(unix))]
    if manifest_broker.is_some()
        || std::env::var("PROVA_PLACEMENT_BROKER").is_ok_and(|v| !v.trim().is_empty())
    {
        eprintln!("prova: a placement broker is configured, but the placement transport is a unix socket — unavailable on this platform");
        return Err(ExitCode::from(2));
    }
    Ok(())
}

/// The stdout sink chosen by --format, plus an optional JUnit XML *file* sink (--junit), plus the
/// GitHub Actions sink — fanned out through a MultiReporter so a CI run can print to the console
/// and drop a results.xml at once, wrapped in the FailureRecorder that feeds `--last-failed` and
/// the run record.
fn build_reporter(
    cli: &Cli,
    env: &RunEnv,
    home: &Option<Home>,
) -> Result<FailureRecorder, ExitCode> {
    // Color resolution, per key: CLI flag > `PROVA_COLOR` env > manifest > auto. Format never
    // auto-switches (a piped console run stays console, just uncolored); only color detects.
    let color = cli
        .color
        .or_else(|| {
            std::env::var("PROVA_COLOR")
                .ok()
                .and_then(|v| report::ColorMode::parse(&v))
        })
        .or(env.env.color)
        .unwrap_or(report::ColorMode::Auto);
    // `--quiet` can only *enable* — a flag that silences must not be silently un-silenced.
    let quiet = cli.quiet || env.env.quiet.unwrap_or(false);
    // Displayed source locations relativize against the package root (else the cwd).
    let rel_root = home
        .as_ref()
        .map(|h| h.dir.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut sinks: Vec<Box<dyn Reporter>> = vec![match env.env.format {
        Format::Console => Box::new(report::HumanReporter::new(color, quiet, rel_root)),
        Format::Json => Box::new(JsonReporter::new(std::io::stdout())),
        Format::Tap => Box::new(TapReporter::new(std::io::stdout())),
    }];
    // `--junit PATH` wins over the manifest's `junit` key (a home-relative path, so a CI profile
    // needs no extra flag). The suite is named after the package (the home directory's basename),
    // and run metadata rides along as `<properties>`.
    let junit_path: Option<PathBuf> = cli
        .junit
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| env.env.junit.as_ref().map(|p| env.base_dir.join(p)));
    if let Some(path) = &junit_path {
        let suite_name = home
            .as_ref()
            .and_then(|h| h.dir.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("prova");
        let mut properties = vec![
            ("prova.version".to_string(), PROVA_VERSION.to_string()),
            ("prova.jobs".to_string(), env.env.jobs.to_string()),
        ];
        if let Some(name) = &cli.profile {
            properties.push(("prova.profile".to_string(), name.clone()));
        }
        match std::fs::File::create(path) {
            Ok(file) => sinks.push(Box::new(
                JUnitReporter::new(file, suite_name).with_properties(properties),
            )),
            Err(e) => {
                eprintln!("prova: cannot open junit report file {path:?}: {e}");
                return Err(ExitCode::from(2));
            }
        }
    }
    // The GitHub Actions sink (CLI > PROVA_GHA env > manifest > auto). An *additional* sink:
    // annotations + step summary compose with whatever --format prints.
    let gha = cli
        .gha
        .or_else(|| {
            std::env::var("PROVA_GHA")
                .ok()
                .and_then(|v| report::GhaMode::parse(&v))
        })
        .or(env.env.github)
        .unwrap_or(report::GhaMode::Auto);
    if gha.enabled() {
        sinks.push(Box::new(report::GitHubReporter::from_env()));
    }
    // Record failed node paths so the next `--last-failed` can re-run exactly them, and every
    // leaf's outcome so the run record can say what did NOT run.
    Ok(FailureRecorder {
        inner: Box::new(MultiReporter::new(sinks)),
        failed: Vec::new(),
        executed: std::collections::BTreeMap::new(),
        skipped: Vec::new(),
    })
}

/// The shared tracking registries a run feeds and the conclusion drains.
struct Accounts {
    // The deputed account (docs/design/verifiers.md): every case a verifier facet ingests
    // (`junit.verify`) accumulates here, drained into the run record.
    deputed: prova_core::DeputedRegistry,
    // The measurement account (docs/design/verifiers.md): every scalar a `measure.record`/
    // `measure.ratchet` call takes accumulates here, drained into the record and, under
    // `--update-baseline`, into the guarded baseline writer.
    measurements: prova_core::MeasurementRegistry,
    attached: prova_core::AttachedRegistry,
    snapshots: Option<prova_core::SnapshotRegistry>,
}

/// Recover a poisoned account lock: every account is a plain collection, valid at every step.
fn drain<T: Default>(m: &std::sync::Mutex<T>) -> T {
    std::mem::take(&mut *m.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Evaluate the attention account and store the run record (var/ always; `--record` also).
#[allow(clippy::too_many_arguments)]
fn store_run_record(
    cli: &Cli,
    home: &Option<Home>,
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    summary: &prova_core::Summary,
    reporter: &mut FailureRecorder,
    accounts: &Accounts,
    full_run: bool,
    measurements: &[prova_core::Measurement],
) -> Vec<record::ReminderEntry> {
    // The attention account (docs/design/reminders.md): conditions evaluate HERE — during
    // the run, in a phase after the proofs — and only against a FULL manifest run, the same
    // soundness rule as --unreferenced (a selection, --promises, or --falsify produces a
    // partial account, and a partial `failed == 0` would fire ledger conditions early).
    // Any other run carries the previous record's rows forward, so a `-k` run can never
    // wipe the account; a full run with no declarations writes it empty (deleted reminders
    // must vanish).
    let reminders: Vec<record::ReminderEntry> = match &home {
        Some(h) if full_run => {
            if summary.reminders_declared > 0 {
                evaluate_run_reminders(h, suites, config, summary, measurements)
            } else {
                Vec::new()
            }
        }
        Some(h) => record::load(h).map(|r| r.reminders).unwrap_or_default(),
        None => Vec::new(),
    };

    record::store(
        home,
        &record::Record {
            // 2: the open-promise executed value is `"promised"` (was `"spec"` in schema 1;
            // `Executed`'s `alias = "spec"` still reads an old record until the next run).
            schema: 2,
            version: env!("CARGO_PKG_VERSION").to_string(),
            binary: record::binary_fingerprint(),
            selection: spell_selection(config),
            duration_ms: summary.duration.as_millis() as u64,
            summary: record::Counts {
                passed: summary.passed,
                failed: summary.failed,
                skipped: summary.skipped,
                promised: summary.promised,
                deselected: summary.deselected,
            },
            executed: std::mem::take(&mut reporter.executed),
            skipped: std::mem::take(&mut reporter.skipped),
            deselected: summary.deselected_paths.clone(),
            reminders: reminders.clone(),
            deputed: record::deputed_rows(&drain(&accounts.deputed)),
            measurements: record::measurement_rows(measurements),
            attached: accounts
                .attached
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        },
        cli.record_to.as_deref(),
    );
    reminders
}

/// An explicit selection that matched NOTHING is an error, not a green run.
///
/// The selection axis's instance of the contract: `-k` is *intent*, and a run that asked
/// for something and got nothing did not succeed at it — it usually means a typo, and a
/// typo must not be green. (Distinct from `requires`, which is *ability*: that skips, and
/// is a declared hole rather than a mistake.) Exit 2, with the other usage errors: nothing
/// failed a test.
fn report_empty_selection(selection: &prova_core::Selection, deselected: usize) {
    let mut asked: Vec<String> = Vec::new();
    asked.extend(selection.keywords.iter().map(|k| format!("-k {k:?}")));
    asked.extend(selection.tags.iter().map(|t| format!("--tags {t:?}")));
    asked.extend(selection.nodes.iter().map(|n| format!("--node {n:?}")));
    if !selection.lane_tags.is_empty() || !selection.lane_tag_excludes.is_empty() {
        asked.push(format!(
            "lane tags {:?}",
            selection
                .lane_tags
                .iter()
                .cloned()
                .chain(selection.lane_tag_excludes.iter().map(|t| format!("!{t}")))
                .collect::<Vec<_>>()
        ));
    }
    eprintln!(
        "prova: selection matched no tests ({}) — {} deselected",
        asked.join(", "),
        deselected
    );
    eprintln!(
        "prova: a selection that matches nothing is usually a typo; pass --allow-empty if \
         selecting nothing is intended here."
    );
}

/// Everything after the suites ran: the record, the topology-attach decree, baselines, the
/// empty-selection error, the attention section, snapshot reconciliation, and the exit code.
#[allow(clippy::too_many_arguments)]
fn conclude_run(
    cli: &Cli,
    home: &Option<Home>,
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    summary: prova_core::Summary,
    mut reporter: FailureRecorder,
    accounts: Accounts,
    env_heed: crate::manifest::Heed,
    from_manifest: bool,
    is_console: bool,
) -> ExitCode {
    store_last_failed(home, &reporter.failed);

    // Drain this run's measurements once, up front: they feed the attention account (a
    // reminder condition can read them — the pre-authorship surface of the same claim a
    // ratchet gates), the record (history), and the guarded baseline writer below.
    let measurements = drain(&accounts.measurements);

    let full_run =
        from_manifest && config.selection.is_empty() && !cli.falsify && !cli.promises_only;
    let reminders = store_run_record(
        cli, home, suites, config, &summary, &mut reporter, &accounts, full_run, &measurements,
    );

    // `--topology NAME` insisted on attaching — a suite that never declared the topology
    // ran against nothing held, which is exactly what the flag exists to prevent.
    if let Some(want) = &cli.require_topology {
        let bound = accounts
            .attached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|n| n == want);
        if !bound {
            eprintln!(
                "prova: --topology {want:?}: the suite never declared it (prova.topology({want:?}, …)) — nothing ran against the held instance"
            );
            return ExitCode::from(2);
        }
    }

    // `--update-baseline`: move the committed baselines toward this run's observed values,
    // through the guard (tightens freely; refuses to loosen). Never happens on a plain run.
    if cli.update_baseline {
        match home.as_ref() {
            Some(h) => {
                prova_core::baselines::update(&h.dir, &measurements).print();
            }
            None => eprintln!("prova: --update-baseline: no project home; nothing to write"),
        }
    }

    // Open promises COUNT as matched: `--node "<a promised test>"` selected and ran that
    // node — its body being expectedly red is the promise mechanism, not an empty
    // selection. Field-reported: the error fired after a PROMISED node was plainly shown.
    let ran = summary.passed + summary.failed + summary.skipped + summary.promised;
    if ran == 0 && !config.selection.is_empty() && !cli.allow_empty {
        report_empty_selection(&config.selection, summary.deselected);
        return ExitCode::from(2);
    }

    // The attention section, after the evidence summary — console only, and only when
    // freshly evaluated (a carried-forward account was already reported by the run that
    // evaluated it; re-printing it here would date-stamp stale news as this run's).
    if is_console && full_run {
        print_reminders(&reminders);
    }

    // Reconcile unreferenced snapshots (only when tracking was enabled on a full run).
    let orphaned = reconcile_unreferenced(accounts.snapshots.as_ref(), &cli.unreferenced);

    // DUE is non-fatal by default — the world moving is not a defect in the change under
    // test. A context that promised attention fails on the DUE reminders it heeds: `heed` in
    // the manifest / a profile, plus this invocation's `--heed`, unioned. Selective heed
    // (`heed = ["line-counts"]`, `--heed=clippy`) gates only the matching DUE reminders.
    let effective_heed = env_heed.merge(cli.heed.clone());
    let heeded_due = reminders
        .iter()
        .filter(|e| e.is_due() && effective_heed.heeds(e))
        .count();
    if heeded_due > 0 {
        let plural = if heeded_due == 1 { "" } else { "s" };
        eprintln!(
            "prova: {heeded_due} heeded reminder{plural} due — this context heeds the \
             attention account (heed / --heed); see `prova reminders`"
        );
        return ExitCode::FAILURE;
    }

    if summary.is_success() && !(cli.unreferenced == "warn" && orphaned) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Build the suites to run (declared `[suites.*]` first, then the plain paths).
fn collect_run_suites(env: &RunEnv, from_manifest: bool) -> Result<Vec<prova_core::Suite>, ExitCode> {
    let suites = match collect_suites(&env.base_dir, &env.env.suites, &env.paths, from_manifest) {
        Ok(suites) => suites,
        Err(msg) => {
            eprintln!("prova: {msg}");
            return Err(ExitCode::from(2));
        }
    };
    if suites.is_empty() {
        eprintln!("prova: no declaration files found (looked for *.prova.lua, plus the accepted *_test.lua / *.test.lua)");
        if let Some(hint) = stray_proof_hint(&env.base_dir, &env.paths) {
            eprintln!("prova: {hint}");
        }
        return Err(ExitCode::from(2));
    }
    Ok(suites)
}

/// `--unreferenced warn|delete`: track referenced `.snap` files so we can reconcile orphans after
/// the run. Sound only on a **full** run — a selection (`-k`/`--tags`/`--node`/`--last-failed`)
/// would make unrun tests' snapshots look orphaned — so skip (with a note) when a filter is active.
fn track_snapshots(
    cli: &Cli,
    config: &mut prova_core::RunConfig,
) -> Option<prova_core::SnapshotRegistry> {
    if cli.unreferenced == "ignore" {
        return None;
    }
    if !config.selection.is_empty() {
        eprintln!(
            "prova: --unreferenced is skipped on a filtered run (it needs the full suite to be sound)"
        );
        return None;
    }
    let reg: prova_core::SnapshotRegistry =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
    *config = std::mem::take(config).with_snapshot_tracking(reg.clone());
    Some(reg)
}

/// The attention account, listed: every declared reminder, with the state the last run
/// recorded overlaid (or "—" for one no run has evaluated yet). Collection loads the suite
/// but executes nothing — so `prova reminders` works before a run and fills in after.
fn reminders_listing(
    suites: &[prova_core::Suite],
    config: &prova_core::RunConfig,
    home: &Option<Home>,
    state: Option<&str>,
) -> ExitCode {
    let declared = prova_core::collect_reminders(suites, config);
    // A selection that matched nothing gets its own honest line — the courtesy message below
    // ("no reminders declared") would misread a narrow filter as an undeclared lane.
    if declared.is_empty() && !config.selection.is_empty() {
        println!("prova: no reminder matches the selection");
        return ExitCode::SUCCESS;
    }
    let recorded = home
        .as_ref()
        .and_then(record::load)
        .map(|r| r.reminders)
        .unwrap_or_default();
    list_reminders(&declared, &recorded, state)
}

/// The resolution prologue: filesystem layout, the prova home, the environment (manifest answers
/// or ad-hoc defaults), and the `-P` package layer.
fn resolve_run(cli: &mut Cli) -> Result<(XdgSystemLayout, Option<Home>, RunEnv), ExitCode> {
    // Filesystem layout — where global plugins live (data_dir/plugins) and where git plugins cache.
    let layout = match XdgSystemLayout::new() {
        Ok(layout) => layout,
        Err(err) => {
            eprintln!("prova: cannot determine home directories: {err}");
            return Err(ExitCode::from(2));
        }
    };
    let home = resolve_run_home(cli)?;
    let mut env = resolve_env(cli, &home, &layout)?;
    // Ad-hoc `-P name=source` entries (e.g. CI-only extras) resolve the same way as manifest
    // plugins and layer on top, overriding a manifest plugin of the same name.
    layer_cli_packages(&cli.packages, &layout, &env.env.sources, &mut env.env.dependencies)?;
    Ok((layout, home, env))
}

pub(crate) fn run(cli_args: Vec<String>) -> ExitCode {
    let mut cli = match parse_cli(cli_args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let from_manifest = cli.explicit_paths.is_empty();
    let (layout, home, mut env) = match resolve_run(&mut cli) {
        Ok(resolved) => resolved,
        Err(code) => return code,
    };
    let suites = match collect_run_suites(&env, from_manifest) {
        Ok(suites) => suites,
        Err(code) => return code,
    };
    let progress_sink = match resolve_progress(&cli, &env.env) {
        Ok(mode) => progress::sink(mode),
        Err(code) => return code,
    };

    // The standalone `prova` binary ships the archetect plugin, so `archetect.render{...}` works.
    // The plugin searcher consults the global install dir plus any manifest-declared plugins.
    let deputed_registry: prova_core::DeputedRegistry = std::sync::Arc::default();
    let measurement_registry: prova_core::MeasurementRegistry = std::sync::Arc::default();
    let mut config = match build_config(
        &mut cli,
        &mut env,
        &home,
        &progress_sink,
        &deputed_registry,
        &measurement_registry,
    ) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let attached_registry = match attach_held(&cli, &home, config) {
        Ok((c, registry)) => {
            config = c;
            registry
        }
        Err(code) => return code,
    };

    let snapshot_registry = track_snapshots(&cli, &mut config);

    // IDE integration: on a manifest run (not a read-only discovery — `--list`, or the `backfill`
    // gate), refresh the annotation folder (core + plugin `---@meta` stubs) and manage `.luarc.json`
    // per `[luals] manage`, so `require("<plugin>")` completes in the editor with no manual wiring.
    // Never blocks the run — a sync error is a warning, not a failure — and all output goes to stderr
    // so `--format json` stdout stays a clean event stream.
    if !cli.list && !cli.backfill {
        if let Some(home) = &home {
            match annotations::setup(home, &env.env.dependencies.roots, env.env.manage, &layout, PROVA_VERSION)
            {
                Ok(outcome) => report_annotations(&outcome),
                Err(err) => eprintln!("prova: IDE annotations: {err}"),
            }
        }
    }

    // The query verbs answer from the collection (bodies never execute) and exit here.
    if cli.switches_list {
        return switches_listing(&suites, &config, &home);
    }
    if cli.reminders_list {
        return reminders_listing(&suites, &config, &home, cli.reminders_state);
    }
    if cli.backfill {
        return backfill_listing(&suites, &config);
    }
    if cli.list {
        return nodes_listing(&suites, &config, cli.list_tagged);
    }

    if let Err(code) = placement_handshake(env.env.placement_broker.as_deref()) {
        return code;
    }

    // Remembered before the reporter takes `format`: the attention section prints only on the
    // console — the JSON/TAP streams are the evidence account and never carry reminders.
    let is_console = matches!(env.env.format, Format::Console);
    let reporter = match build_reporter(&cli, &env, &home) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let accounts = Accounts {
        deputed: deputed_registry,
        measurements: measurement_registry,
        attached: attached_registry,
        snapshots: snapshot_registry,
    };

    let mut reporter = reporter;
    match run_suites(&suites, &mut reporter, &config) {
        Ok(summary) => conclude_run(
            &cli,
            &home,
            &suites,
            &config,
            summary,
            reporter,
            accounts,
            env.env.heed.clone(),
            from_manifest,
            is_console,
        ),
        Err(err) => {
            eprintln!("prova: {err}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, ExitCode> {
        parse_cli(args.iter().map(|s| s.to_string()).collect())
    }

    /// The run-shaping grammar in one pass: every selection axis with its `!` exclusion form,
    /// comma-splitting where lists are natural, and the value flags that name what runs.
    #[test]
    fn parse_cli_reads_the_selection_axes_and_run_shape() {
        let cli = parse(&[
            "-k", "orders", "-k", "!slow-orders",
            "--tags", "unit, http ,!flaky",
            "--node", "engine › selects",
            "-j", "4", "-p", "quality", "-s", "ci,docker", "--switch", "soak",
            "-P", "pg=./packages/pg", "--record", "out.json", "--topology", "dev",
            "--heed=ops,security", "--heed=ops",
            "proofs/engine",
        ])
        .ok()
        .unwrap();
        assert_eq!(cli.selection.keywords, vec!["orders"]);
        assert_eq!(cli.selection.keyword_excludes, vec!["slow-orders"]);
        assert_eq!(cli.selection.tags, vec!["unit", "http"]);
        assert_eq!(cli.selection.tag_excludes, vec!["flaky"]);
        assert_eq!(cli.selection.nodes, vec!["engine › selects"]);
        assert_eq!(cli.jobs, Some(4));
        assert_eq!(cli.profile.as_deref(), Some("quality"));
        assert_eq!(cli.switches, vec!["ci", "docker", "soak"]);
        assert_eq!(cli.packages, vec!["pg=./packages/pg"]);
        assert_eq!(cli.record_to.as_deref(), Some(std::path::Path::new("out.json")));
        assert_eq!(cli.require_topology.as_deref(), Some("dev"));
        assert_eq!(
            cli.heed,
            crate::manifest::Heed::Matching(vec!["ops".into(), "security".into()]),
            "heed selectors accumulate and dedupe across flags"
        );
        assert_eq!(cli.explicit_paths, vec!["proofs/engine"], "bare paths are the selection");
    }

    /// Every taught refusal exits 2 at the parse, before anything loads: a non-positive job
    /// count, and unknown --format/--color/--progress spellings.
    #[test]
    fn parse_cli_refuses_bad_values_at_the_door() {
        for bad in [
            &["--jobs", "0"][..],
            &["--jobs", "many"][..],
            &["--format", "yaml"][..],
            &["--color", "sometimes"][..],
            &["--progress", "maybe"][..],
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be refused");
        }
        let cli = parse(&["--format", "json", "--color", "never", "--progress", "always"])
            .ok()
            .unwrap();
        assert!(matches!(cli.format, Some(Format::Json)));
        assert!(matches!(cli.color, Some(report::ColorMode::Never)));
    }
}
