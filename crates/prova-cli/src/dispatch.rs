//! The dispatcher: `run(cli_args)` — flag parsing and the verb switch.

use super::*;

pub(crate) fn run(cli_args: Vec<String>) -> ExitCode {
    let mut cli_format: Option<Format> = None;
    let mut cli_color: Option<report::ColorMode> = None;
    let mut cli_progress: Option<progress::Mode> = None;
    let mut cli_quiet = false;
    let mut cli_heed = crate::manifest::Heed::None;
    let mut cli_switches: Vec<String> = Vec::new();
    let mut cli_junit: Option<String> = None;
    let mut cli_gha: Option<report::GhaMode> = None;
    let mut cli_jobs: Option<usize> = None;
    let mut update_snapshots = false;
    let mut update_baseline = false;
    let mut unreferenced = String::from("ignore"); // ignore | warn | delete
    let mut cli_config: Option<String> = None;
    let mut list = false;
    // Internal: `prova tests` routes here as `--list --list-tagged` so the listing renders each node
    // with its promise⇄proof state (PROMISE / PROOF), while plain `--list` (and the retiring
    // `prova list`) stays bare paths. Not a user-facing flag.
    let mut list_tagged = false;
    // Internal: `prova specs backfill` routes here to list the proofs no claim backs (empty `covers`)
    // — the reverse of `owed`, a red→green worklist that gates (exit ≠0 while any is unbacked). Not a
    // user-facing flag.
    let mut backfill = false;
    // Internal: `prova reminders` routes here to COLLECT declared reminders (loading the suite,
    // like `--list`, without executing) and overlay the recorded state — so the verb works before
    // any run and shows live states after one. Not a user-facing flag.
    let mut reminders_list = false;
    let mut reminders_state: Option<&str> = None;
    let mut switches_list = false;
    let mut promises_only = false;
    let mut proofs_only = false;
    let mut falsify = false;
    // Held-topology attach (docs/design/topologies.md#attach-binds-by-name): `--fresh` opts a run
    // out of attaching to held topologies; `--topology NAME` insists on attaching to NAME.
    let mut fresh = false;
    let mut require_topology: Option<String> = None;
    let mut due = false;
    let mut explicit_paths: Vec<String> = Vec::new();
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut cli_packages: Vec<String> = Vec::new();
    let mut selection = prova_core::Selection::default();
    let mut last_failed = false;
    let mut record_to: Option<std::path::PathBuf> = None;
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
        // `-P name=source` (repeatable): an ad-hoc package, layered over the manifest (CLI wins).
        if let Some(v) = value_flag(&arg, &mut args, &["--package", "-P", "--plugin"]) {
            if arg.starts_with("--plugin") { eprintln!("prova: `--plugin` is deprecated — use `--package` (retires at 1.0)"); }
            cli_packages.push(v);
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
        // `-s class` / `--switch a,b` (repeatable): throw opt-in switches — authorize the named
        // classes for this run (docs/design/manifest.md#switches-not-env-capabilities). Unions
        // with `[run]`/profile `switches`; a throw authorizes, it never widens a profile's scope.
        if let Some(v) = value_flag(&arg, &mut args, &["--switch", "-s"]) {
            for s in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                cli_switches.push(s.to_string());
            }
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
        // `--progress auto|always|never`: report what a long pause IS (pulling, waiting) on
        // stderr. Separate from `--color` because it is a different stream and a different concern:
        // color styles stdout, progress narrates stderr, and neither can corrupt a machine format.
        if let Some(v) = value_flag(&arg, &mut args, &["--progress"]) {
            match progress::Mode::parse(&v) {
                Ok(mode) => cli_progress = Some(mode),
                Err(e) => {
                    eprintln!("prova: {e}");
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        // `--topology NAME`: REQUIRE attaching to the held topology NAME — error rather than
        // silently provisioning fresh, because a run meant to judge a live environment (e.g. a
        // Tilt-injected work-in-progress build) must never quietly test something else.
        if let Some(v) = value_flag(&arg, &mut args, &["--topology"]) {
            require_topology = Some(v);
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
        // `--gha auto|on|off`: the GitHub Actions sink (annotations + step summary).
        if let Some(v) = value_flag(&arg, &mut args, &["--gha"]) {
            match report::GhaMode::parse(&v) {
                Some(mode) => cli_gha = Some(mode),
                None => {
                    eprintln!("prova: unknown --gha {v:?} (expected auto|on|off)");
                    return ExitCode::from(2);
                }
            }
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
        // `--record <path>`: ALSO emit the run record here. The var/ copy is written either way and
        // is for the next command; this one is for CI to keep as an artifact, or a human to read.
        if let Some(v) = value_flag(&arg, &mut args, &["--record"]) {
            record_to = Some(std::path::PathBuf::from(v));
            continue;
        }
        // `--heed` (bare) heeds all DUE reminders; `--heed=<sel>[,<sel>]` heeds only the matching
        // ones (by reminder name/tag) — the ad-hoc form of a profile's `heed` list. Accumulates.
        if let Some(sels) = arg.strip_prefix("--heed=") {
            let list: Vec<String> = sels
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            cli_heed = std::mem::take(&mut cli_heed).merge(crate::manifest::Heed::Matching(list));
            continue;
        }
        match arg.as_str() {
            "--list" => list = true,
            "--list-tagged" => {
                list = true;
                list_tagged = true;
            }
            "--backfill" => backfill = true,
            "--reminders-list" => reminders_list = true,
            "--switches-list" => switches_list = true,
            // Internal spellings of the reminders lane's state filters (`prova reminders --due`);
            // rewritten by reminders_subcommand because bare `--due` is the promise decree below.
            "--reminders-due" => reminders_state = Some("due"),
            "--reminders-watching" => reminders_state = Some("watching"),
            "--quiet" | "-q" => cli_quiet = true,
            // Promote this one invocation to heed the attention account — the ad-hoc form of the
            // manifest's `heed`; like every guarantee it can only tighten (All absorbs).
            "--heed" => cli_heed = crate::manifest::Heed::All,
            "--last-failed" => last_failed = true,
            "--falsify" => falsify = true,
            "--fresh" => fresh = true,
            "--promises" => promises_only = true,
            "--proofs" => proofs_only = true,
            "--due" => due = true,
            "--allow-empty" => allow_empty = true,
            "--update-snapshots" | "-u" => update_snapshots = true,
            "--update-baseline" => update_baseline = true,
            "--update" | "-U" => update_force = true,
            "--offline" => offline = true,
            "--json" => cli_format = Some(Format::Json),
            "--version" | "-V" => {
                println!("prova {}", prova_core::VERSION);
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

    // Determine the prova home (the directory owning `prova.toml`). `--manifest PATH` points
    // directly at a manifest; a manifest-mode run walks up from the current directory; explicit
    // path args anchor discovery at the NAMED paths themselves — a file selects what to run but
    // keeps its package's environment, even when named from outside the package. An ambiguous
    // layout (more than one manifest location) is an error, as is a selection spanning packages.
    let home: Option<Home> = if let Some(path) = &manifest_path {
        Some(home::from_manifest_path(Path::new(path)))
    } else if !explicit_paths.is_empty() {
        match home_for_explicit_paths(&explicit_paths) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        match home::find(Path::new(".")) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::from(2);
            }
        }
    };

    // Resolve the run. Explicit path args are the SELECTION (literal paths relative to cwd, no
    // declared-suite fan-out, no IDE management) but never strip the package environment: when the
    // named paths live in a package, its manifest still supplies plugins, capabilities, and run
    // defaults. Otherwise read the home's `prova.toml` (a `proofs` name-pattern rooted at home).
    let from_manifest = explicit_paths.is_empty();
    let (
        base_dir,
        paths,
        jobs,
        format,
        manifest_color,
        manifest_progress,
        manifest_quiet,
        manifest_gha,
        manifest_junit,
        declared,
        mut packages_resolved,
        sources,
        manage,
        capabilities,
        globals_inject,
        manifest_broker,
        heed,
        lane_tags,
        manifest_switches,
    ) = if !explicit_paths.is_empty() {
        match &home {
            // The named paths belong to a package: borrow its environment (plugins, capabilities,
            // jobs/format defaults — CLI flags still win inside resolve_from_manifest), while the
            // selection stays the explicit paths and `[suites.*]` declarations do not fan out.
            Some(home) => match resolve_from_manifest(
                home,
                profile.clone(),
                cli_jobs,
                cli_format,
                cli_config,
                &layout,
                update_force,
                offline,
                false,
            ) {
                Ok(r) => (
                    PathBuf::from("."),
                    explicit_paths,
                    r.jobs,
                    r.format,
                    r.color,
                    r.progress,
                    r.quiet,
                    r.github,
                    r.junit,
                    BTreeMap::new(),
                    r.dependencies,
                    r.sources,
                    Manage::Never,
                    r.capabilities,
                    r.globals_inject,
                    r.placement_broker,
                    r.heed,
                    r.lane_tags,
                    r.switches,
                ),
                Err(code) => return code,
            },
            // No package anywhere above the named paths: an ad-hoc run with built-ins only.
            None => (
                PathBuf::from("."),
                explicit_paths,
                cli_jobs.unwrap_or(1),
                cli_format.unwrap_or(Format::Console),
                None, // color
                None, // progress
                None, // quiet
                None, // github
                None, // junit
                BTreeMap::new(),
                packages::ResolvedPackages::default(),
                BTreeMap::new(),
                Manage::Never,
                // No manifest, so there is no companion — built-in capabilities still work;
                // registered ones are simply absent.
                prova_core::Capabilities::default(),
                Vec::new(),
                None,       // no manifest, no [placement]; the env var is still honoured below
                crate::manifest::Heed::None, // heed — no manifest, nothing promised attention
                Vec::new(), // lane tags — no manifest, no lanes
                Vec::new(), // switches — no manifest, nothing thrown
            ),
        }
    } else {
        let Some(home) = &home else {
            eprintln!(
                "usage: prova <file-or-dir>...   or   prova [--profile NAME]  (reads prova.toml)"
            );
            return ExitCode::from(2);
        };
        match resolve_from_manifest(
            home,
            profile.clone(),
            cli_jobs,
            cli_format,
            cli_config,
            &layout,
            update_force,
            offline,
            true,
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
                r.progress,
                r.quiet,
                r.github,
                r.junit,
                r.suites,
                r.dependencies,
                r.sources,
                r.manage,
                r.capabilities,
                r.globals_inject,
                r.placement_broker,
                r.heed,
                r.lane_tags,
                r.switches,
            ),
            Err(code) => return code,
        }
    };

    // Ad-hoc `-P name=source` entries (e.g. CI-only extras) resolve the same way as manifest
    // plugins and layer on top, overriding a manifest plugin of the same name.
    if let Err(code) = layer_cli_packages(&cli_packages, &layout, &sources, &mut packages_resolved) {
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
        eprintln!("prova: no declaration files found (looked for *.prova.lua, plus the accepted *_test.lua / *.test.lua)");
        if let Some(hint) = stray_proof_hint(&base_dir, &paths) {
            eprintln!("prova: {hint}");
        }
        return ExitCode::from(2);
    }

    // Progress resolution mirrors color's ladder: CLI flag > `PROVA_PROGRESS` > manifest > auto.
    // A bad env value is a hard error rather than a silent fallback — a typo'd `PROVA_PROGRESS=nevr`
    // that quietly leaves activity ON is the sort of thing someone debugs for ten minutes.
    let progress_mode = match cli_progress {
        Some(m) => m,
        None => match std::env::var("PROVA_PROGRESS") {
            Ok(v) if !v.trim().is_empty() => match progress::Mode::parse(&v) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("prova: {e} (from PROVA_PROGRESS)");
                    return ExitCode::from(2);
                }
            },
            _ => manifest_progress.unwrap_or(progress::Mode::Auto),
        },
    };
    // `--quiet` silences the reporter's stdout chatter; it silences activity too, because someone
    // asking for quiet means it. `--progress always` is the way back if you want narration without
    // the tree.
    let quiet_for_progress = cli_quiet || manifest_quiet.unwrap_or(false);
    let progress_mode = if quiet_for_progress && cli_progress.is_none() {
        progress::Mode::Never
    } else {
        progress_mode
    };
    let progress_sink = progress::sink(progress_mode);

    // The standalone `prova` binary ships the archetect plugin, so `archetect.render{...}` works.
    // The plugin searcher consults the global install dir plus any manifest-declared plugins.
    // The deputed account (docs/design/verifiers.md): every case a verifier facet ingests
    // (`junit.verify`) accumulates here, drained into the run record below.
    let deputed_registry: prova_core::DeputedRegistry = std::sync::Arc::default();
    // The measurement account (docs/design/verifiers.md): every scalar a `measure.record`/
    // `measure.ratchet` call takes accumulates here, drained below into the record and, under
    // `--update-baseline`, into the guarded baseline writer.
    let measurement_registry: prova_core::MeasurementRegistry = std::sync::Arc::default();
    if promises_only && proofs_only {
        eprintln!(
            "prova: --promises and --proofs are mutually exclusive — a test is a promise or a proof, \
             not both"
        );
        return ExitCode::from(2);
    }
    let mut config = engine_config(jobs, &packages_resolved, home.as_ref(), std::sync::Arc::clone(&progress_sink))
        .with_update_snapshots(update_snapshots)
        .with_due(due)
        .with_promises_only(promises_only)
        .with_proofs_only(proofs_only)
        .with_falsify(falsify)
        // Thrown switches: the manifest's ([run] ∪ profile) ∪ the CLI's `-s` — all doors union
        // (docs/design/manifest.md#switches-not-env-capabilities).
        .with_switches(manifest_switches.iter().cloned().chain(cli_switches.iter().cloned()))
        .with_capabilities(capabilities)
        .with_globals_inject(globals_inject)
        .with_deputed_tracking(deputed_registry.clone())
        .with_measurement_tracking(measurement_registry.clone());

    // `--last-failed`: fold the previous run's failed node paths into the selection as exact nodes.
    if last_failed {
        match load_last_failed(&home) {
            Some(paths) if !paths.is_empty() => selection.nodes.extend(paths),
            _ => eprintln!(
                "prova: --last-failed: no failure state from a previous run here; running everything"
            ),
        }
    }
    // The lane's baked tags join the selection as their own gate (`!` splits into excludes,
    // same grammar as --tags). The CLI's axes then narrow WITHIN the lane.
    for t in &lane_tags {
        match t.strip_prefix('!') {
            Some(rest) => selection.lane_tag_excludes.push(rest.to_string()),
            None => selection.lane_tags.push(t.clone()),
        }
    }
    config.selection = selection;

    // Held-topology attach (docs/design/topologies.md#attach-binds-by-name): unless `--fresh`,
    // every LIVE held record is offered to the run; the engine binds the ones the collection
    // actually declares, by name, instead of provisioning — and reports each into this registry
    // so the run record can carry live-state provenance. Announced up front: an attached run is
    // deliberately non-hermetic, and that must never be silent.
    let attached_registry: prova_core::AttachedRegistry = std::sync::Arc::default();
    if fresh {
        if require_topology.is_some() {
            eprintln!("prova: --topology and --fresh contradict each other — pick one");
            return ExitCode::from(2);
        }
    } else {
        if let Some(h) = &home {
            for rec in runstate::list(h) {
                if !runstate::is_alive(rec.pid) {
                    continue;
                }
                if let Some(want) = &require_topology {
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
        if let Some(want) = &require_topology {
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
                return ExitCode::from(2);
            }
        }
        config = config.with_attached_tracking(attached_registry.clone());
    }

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

    // IDE integration: on a manifest run (not a read-only discovery — `--list`, or the `backfill`
    // gate), refresh the annotation folder (core + plugin `---@meta` stubs) and manage `.luarc.json`
    // per `[luals] manage`, so `require("<plugin>")` completes in the editor with no manual wiring.
    // Never blocks the run — a sync error is a warning, not a failure — and all output goes to stderr
    // so `--format json` stdout stays a clean event stream.
    if !list && !backfill {
        if let Some(home) = &home {
            match annotations::setup(
                home,
                &packages_resolved.roots,
                manage,
                &layout,
                PROVA_VERSION,
            ) {
                Ok(outcome) => report_annotations(&outcome),
                Err(err) => eprintln!("prova: IDE annotations: {err}"),
            }
        }
    }

    if switches_list {
        // The opt-in classes, listed: the census from collection (bodies never execute), the
        // thrown-by column from the manifest — so an ad-hoc-only class is a stated fact, never a
        // hidden test population (docs/design/manifest.md#switches-are-discoverable).
        let census = prova_core::collect_switch_census(&suites, &config);
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
        return ExitCode::SUCCESS;
    }

    if reminders_list {
        // The attention account, listed: every declared reminder, with the state the last run
        // recorded overlaid (or "—" for one no run has evaluated yet). Collection loads the suite
        // but executes nothing — so `prova reminders` works before a run and fills in after.
        let declared = prova_core::collect_reminders(&suites, &config);
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
        return list_reminders(&declared, &recorded, reminders_state);
    }

    if backfill {
        // `prova specs backfill` — the reverse of `owed`: every proof no claim backs (empty
        // `covers`). A red→green worklist that GATES — exit non-zero while any proof is unbacked —
        // so an agent can drive coverage to complete. It never writes a spec: it names the proof and
        // the agent infers the claim (an auto-stubbed `<!-- claim -->` would be vacuous prose).
        let mut unbacked: Vec<String> = Vec::new();
        for suite in &suites {
            match discover_suite(suite, &config) {
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
        return ExitCode::from(1);
    }

    if list {
        // Per-suite, never per-file: the setup (`suite.lua`) must load first so the listed
        // collection is exactly what a run would collect (suite-level `spec`/`requires`/name).
        for suite in &suites {
            match discover_suite(suite, &config) {
                // `prova tests` (`--list-tagged`) state-tags each node with its side of the
                // promise⇄proof duality; plain `--list` stays bare paths (stable, machine-friendly).
                Ok(nodes) if list_tagged => nodes.iter().for_each(|n| {
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
        return ExitCode::SUCCESS;
    }

    // Placement (docs/design/placement.md §Transport): if a broker is configured — env var over
    // manifest — dial it and say hello BEFORE anything runs. Configured-but-unreachable is a loud
    // error, never a silent fall back to local: falling back would turn a broken pool into a
    // suite that quietly stopped distributing, and the only symptom would be that it got slower.
    // (Answering `requires`/`resources` from the pool waits on the dispatch planes — see
    // docs/plans/placement-client.md — so today the handshake validates the configuration and
    // announces the pool; the run itself proceeds as before.)
    #[cfg(unix)]
    if let Some((addr, source)) = placement::configured(manifest_broker.as_deref()) {
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
                return ExitCode::from(2);
            }
        }
    }
    #[cfg(not(unix))]
    if manifest_broker.is_some() || std::env::var("PROVA_PLACEMENT_BROKER").is_ok_and(|v| !v.trim().is_empty()) {
        eprintln!("prova: a placement broker is configured, but the placement transport is a unix socket — unavailable on this platform");
        return ExitCode::from(2);
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
    // Remembered before `format` moves: the attention section prints only on the console — the
    // JSON/TAP streams are the evidence account and never carry reminders.
    let is_console = matches!(format, Format::Console);
    let mut sinks: Vec<Box<dyn Reporter>> = vec![match format {
        Format::Console => Box::new(report::HumanReporter::new(color, quiet, rel_root)),
        Format::Json => Box::new(JsonReporter::new(std::io::stdout())),
        Format::Tap => Box::new(TapReporter::new(std::io::stdout())),
    }];
    // `--junit PATH` wins over the manifest's `junit` key (a home-relative path, so a CI profile
    // needs no extra flag). The suite is named after the package (the home directory's basename),
    // and run metadata rides along as `<properties>`.
    let junit_path: Option<PathBuf> = cli_junit
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| manifest_junit.as_ref().map(|p| base_dir.join(p)));
    if let Some(path) = &junit_path {
        let suite_name = home
            .as_ref()
            .and_then(|h| h.dir.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("prova");
        let mut properties = vec![
            ("prova.version".to_string(), PROVA_VERSION.to_string()),
            ("prova.jobs".to_string(), jobs.to_string()),
        ];
        if let Some(name) = &profile {
            properties.push(("prova.profile".to_string(), name.clone()));
        }
        match std::fs::File::create(path) {
            Ok(file) => sinks.push(Box::new(
                JUnitReporter::new(file, suite_name).with_properties(properties),
            )),
            Err(e) => {
                eprintln!("prova: cannot open junit report file {path:?}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    // The GitHub Actions sink (CLI > PROVA_GHA env > manifest > auto). An *additional* sink:
    // annotations + step summary compose with whatever --format prints.
    let gha = cli_gha
        .or_else(|| {
            std::env::var("PROVA_GHA")
                .ok()
                .and_then(|v| report::GhaMode::parse(&v))
        })
        .or(manifest_gha)
        .unwrap_or(report::GhaMode::Auto);
    if gha.enabled() {
        sinks.push(Box::new(report::GitHubReporter::from_env()));
    }
    // Record failed node paths so the next `--last-failed` can re-run exactly them, and every
    // leaf's outcome so the run record can say what did NOT run.
    let mut reporter = FailureRecorder {
        inner: Box::new(MultiReporter::new(sinks)),
        failed: Vec::new(),
        executed: std::collections::BTreeMap::new(),
        skipped: Vec::new(),
    };

    match run_suites(&suites, &mut reporter, &config) {
        Ok(summary) => {
            store_last_failed(&home, &reporter.failed);

            // Drain this run's measurements once, up front: they feed the attention account (a
            // reminder condition can read them — the pre-authorship surface of the same claim a
            // ratchet gates), the record (history), and the guarded baseline writer below.
            let measurements = std::mem::take(
                &mut *measurement_registry.lock().expect("measurement registry"),
            );

            // The attention account (docs/design/reminders.md): conditions evaluate HERE — during
            // the run, in a phase after the proofs — and only against a FULL manifest run, the same
            // soundness rule as --unreferenced (a selection, --promises, or --falsify produces a
            // partial account, and a partial `failed == 0` would fire ledger conditions early).
            // Any other run carries the previous record's rows forward, so a `-k` run can never
            // wipe the account; a full run with no declarations writes it empty (deleted reminders
            // must vanish).
            let full_run =
                from_manifest && config.selection.is_empty() && !falsify && !promises_only;
            let reminders: Vec<record::ReminderEntry> = match &home {
                Some(h) if full_run => {
                    if summary.reminders_declared > 0 {
                        evaluate_run_reminders(h, &suites, &config, &summary, &measurements)
                    } else {
                        Vec::new()
                    }
                }
                Some(h) => record::load(h).map(|r| r.reminders).unwrap_or_default(),
                None => Vec::new(),
            };

            record::store(
                &home,
                &record::Record {
                    // 2: the open-promise executed value is `"promised"` (was `"spec"` in schema 1;
                    // `Executed`'s `alias = "spec"` still reads an old record until the next run).
                    schema: 2,
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    binary: record::binary_fingerprint(),
                    selection: spell_selection(&config),
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
                    deputed: record::deputed_rows(
                        &std::mem::take(&mut *deputed_registry.lock().expect("deputed registry")),
                    ),
                    measurements: record::measurement_rows(&measurements),
                    attached: attached_registry
                        .lock()
                        .expect("attached registry")
                        .clone(),
                },
                record_to.as_deref(),
            );

            // `--topology NAME` insisted on attaching — a suite that never declared the topology
            // ran against nothing held, which is exactly what the flag exists to prevent.
            if let Some(want) = &require_topology {
                let bound = attached_registry
                    .lock()
                    .expect("attached registry")
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
            if update_baseline {
                match home.as_ref() {
                    Some(h) => {
                        prova_core::baselines::update(&h.dir, &measurements).print();
                    }
                    None => eprintln!("prova: --update-baseline: no project home; nothing to write"),
                }
            }

            // An explicit selection that matched NOTHING is an error, not a green run.
            //
            // The selection axis's instance of the contract: `-k` is *intent*, and a run that asked
            // for something and got nothing did not succeed at it — it usually means a typo, and a
            // typo must not be green. (Distinct from `requires`, which is *ability*: that skips, and
            // is a declared hole rather than a mistake.) Exit 2, with the other usage errors: nothing
            // failed a test.
            //
            // Open promises COUNT as matched: `--node "<a promised test>"` selected and ran that
            // node — its body being expectedly red is the promise mechanism, not an empty
            // selection. Field-reported: the error fired after a PROMISED node was plainly shown.
            let ran = summary.passed + summary.failed + summary.skipped + summary.promised;
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
                if !config.selection.lane_tags.is_empty()
                    || !config.selection.lane_tag_excludes.is_empty()
                {
                    asked.push(format!(
                        "lane tags {:?}",
                        config
                            .selection
                            .lane_tags
                            .iter()
                            .cloned()
                            .chain(
                                config
                                    .selection
                                    .lane_tag_excludes
                                    .iter()
                                    .map(|t| format!("!{t}"))
                            )
                            .collect::<Vec<_>>()
                    ));
                }
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

            // The attention section, after the evidence summary — console only, and only when
            // freshly evaluated (a carried-forward account was already reported by the run that
            // evaluated it; re-printing it here would date-stamp stale news as this run's).
            if is_console && full_run {
                print_reminders(&reminders);
            }

            // Reconcile unreferenced snapshots (only when tracking was enabled on a full run).
            let orphaned = reconcile_unreferenced(snapshot_registry.as_ref(), &unreferenced);

            // DUE is non-fatal by default — the world moving is not a defect in the change under
            // test. A context that promised attention fails on the DUE reminders it heeds: `heed` in
            // the manifest / a profile, plus this invocation's `--heed`, unioned. Selective heed
            // (`heed = ["line-counts"]`, `--heed=clippy`) gates only the matching DUE reminders.
            let effective_heed = heed.merge(cli_heed);
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
