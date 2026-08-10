//! Suite assembly: proof-dir discovery, manifest resolution, engine config.

use super::*;

/// Apply the `--unreferenced` policy to `.snap` files no test referenced this run. `warn` lists them
/// (and the caller fails the run so CI catches rot); `delete` removes them. Returns whether any orphan
/// was found. A no-op when tracking was off (filtered run / policy `ignore`).
pub(crate) fn reconcile_unreferenced(registry: Option<&prova_core::SnapshotRegistry>, policy: &str) -> bool {
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
pub(crate) struct ManifestRun {
    pub(crate) proofs: Vec<String>,
    pub(crate) jobs: usize,
    pub(crate) format: Format,
    /// Manifest `color`/`quiet`/`github` (pre-parsed) — the CLI flags and `PROVA_COLOR`/
    /// `PROVA_GHA` env vars override them at the wiring site.
    pub(crate) color: Option<report::ColorMode>,
    /// Manifest `progress` (pre-parsed) — `--progress` / `PROVA_PROGRESS` override it.
    pub(crate) progress: Option<progress::Mode>,
    pub(crate) quiet: Option<bool>,
    pub(crate) github: Option<report::GhaMode>,
    /// Manifest `junit` (home-relative path) — `--junit` wins.
    pub(crate) junit: Option<String>,
    pub(crate) suites: BTreeMap<String, SuiteDecl>,
    pub(crate) dependencies: packages::ResolvedPackages,
    pub(crate) sources: BTreeMap<String, String>,
    pub(crate) manage: Manage,
    /// Manifest topologies (`[topologies]`) — name → the plugin factory it exposes. Consumed only by
    /// the `up`/`watch`/list verbs, which desugar each to a `prova.topology` registration.
    pub(crate) topologies: BTreeMap<String, crate::manifest::TopologyDecl>,
    /// Capabilities the package's `prova.lua` registered — carried into the run's `RunConfig` so
    /// `requires` resolution sees the same vocabulary the `must_run` precondition just checked. Per
    /// resolve, so the warm MCP's packages don't share.
    pub(crate) capabilities: prova_core::Capabilities,
    /// `[globals] inject` — module names (bundled and/or plugin) bound as unqualified ambient globals.
    pub(crate) globals_inject: Vec<String>,
    /// `[placement] broker` — the manifest half of broker address resolution; the env var wins at
    /// the wiring site. Carried raw: dialing happens once, at run start, not here.
    pub(crate) placement_broker: Option<String>,
    /// Which DUE reminders fail the run — the resolved `heed` policy (`[run]`/a profile's `heed`,
    /// unioned; CLI `--heed` promotes further — see docs/design/reminders.md).
    pub(crate) heed: crate::manifest::Heed,
    /// The lane's baked tag selection (`tags` on `[run]`/the profile) — folded into the run's
    /// Selection as an independent gate the CLI narrows within.
    pub(crate) lane_tags: Vec<String>,
    /// The thrown opt-in switches (`switches` on `[run]`/the profile, unioned) — the CLI's `-s`
    /// unions on top at the wiring site.
    pub(crate) switches: Vec<String>,
}

/// If a linted plugin ships no LuaCATS stub (`library/<canonical>.lua`), return an advisory message.
/// `ns` is `(canonical, plugin_root_dir)` from `namespace_for_file`. Consumers of `require(name)` get
/// no editor completion without a stub — the package archetype generates one; this nudges hand-authored
/// plugins to match.
pub(crate) fn missing_stub_warning(ns: &Option<(String, PathBuf)>) -> Option<String> {
    let (canonical, dir) = ns.as_ref()?;
    let stub = dir.join("library").join(format!("{canonical}.lua"));
    if stub.is_file() {
        return None;
    }
    Some(format!(
        "no library/{canonical}.lua — consumers of require(\"{canonical}\") get no editor \
         completion (add a ---@meta stub; the package archetype generates one)"
    ))
}

/// Print a concise, honest one-liner (to stderr) about what the IDE annotation sync did — and
/// nothing at all when it did nothing. This runs on every invocation; the steady state is silence.
pub(crate) fn report_annotations(outcome: &annotations::Outcome) {
    let linked = if outcome.linked_packages.is_empty() {
        String::new()
    } else {
        format!("; package annotations linked for {}", outcome.linked_packages.join(", "))
    };
    if outcome.luarc_created {
        eprintln!("prova: wrote .luarc.json (editor IDE support enabled{linked})");
    } else if outcome.luarc_updated {
        eprintln!("prova: updated .luarc.json (IDE annotation entries reconciled{linked})");
    } else if outcome.luarc_hint {
        eprintln!(
            "prova: .luarc.json is not plain JSON, so prova cannot merge its IDE annotation \
             entries — add them by hand, or set [luals] manage = \"never\" to silence this"
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
/// Explain an EMPTY discovery, when the reason is visible on disk.
///
/// "no declaration files found (looked for `*.prova.lua`)" reads as a lie when one is plainly sitting there —
/// it is true only in the sense that we did not look *where it is*. Proof files are found in
/// directories NAMED by `[run] proofs`, and several directory names are never scanned at all: the
/// `prova/` / `.prova/` **nook** most of all, because "put prova's files in `prova/`" invites putting
/// the proofs there too (that is exactly how this was found). So when discovery comes up empty, say
/// where the files actually are and what would make them visible.
///
/// Bounded on purpose: a shallow walk that skips the heavy directories, reporting at most a few
/// examples. A hint that costs a full-tree scan on every empty run is a hint that gets removed.
pub(crate) fn stray_proof_hint(root: &Path, patterns: &[String]) -> Option<String> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 || out.len() >= 3 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if out.len() >= 3 {
                return;
            }
            if path.is_dir() {
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else { continue };
                // Skip only what is heavy or never source — NOT the nook, which is the whole point.
                if matches!(name, "target" | "node_modules" | "vendor" | "dist" | "build" | ".git") {
                    continue;
                }
                walk(&path, depth - 1, out);
            } else if prova_core::is_test_file(&path) {
                out.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(root, 5, &mut found);
    if found.is_empty() {
        return None;
    }
    let list = found
        .iter()
        .map(|p| format!("  {}", p.strip_prefix(root).unwrap_or(p).display()))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "found proof file(s) that discovery does not reach:\n{list}\n\
         proofs are discovered in directories NAMED {:?} (`[run] proofs`), anywhere below the package \
         root — and `prova/` / `.prova/` is never scanned, because the nook holds prova's own files \
         while your proof suites live in the open. Move them into a `{}/` directory, or add that \
         directory's name to `[run] proofs`.",
        patterns,
        patterns.first().map(String::as_str).unwrap_or("proofs")
    ))
}

pub(crate) fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "prova" | "target" | "node_modules" | "vendor" | "dist" | "build" | "testdata"
        )
}

/// Whether a directory basename matches one of the `[run] proofs` patterns — a glob when the pattern
/// carries a metacharacter, an exact-name match otherwise.
pub(crate) fn name_matches(name: &str, patterns: &[String]) -> bool {
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
pub(crate) fn find_proof_dirs(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
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

pub(crate) fn expand_pattern(base_dir: &Path, pattern: &str) -> Result<Vec<PathBuf>, String> {
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

/// The prova home for an explicit-path selection: discovery anchors at each NAMED path (a file's
/// directory, a directory itself, a glob's deepest existing ancestor) rather than the cwd, so a
/// file keeps its own package's environment even when named from outside it. All paths must agree
/// on one home — half a run with the wrong plugins is worse than a refusal — and paths outside any
/// package resolve to `None` (an ad-hoc run) only when NO named path belongs to a package.
pub(crate) fn home_for_explicit_paths(paths: &[String]) -> Result<Option<Home>, String> {
    // The deepest existing directory at or above `arg` — a file anchors at its parent; a path that
    // does not exist yet (a glob, a typo caught later at discovery) walks up to solid ground.
    fn anchor(arg: &str) -> PathBuf {
        let p = Path::new(arg);
        let mut dir = if p.is_dir() { p } else { p.parent().unwrap_or(Path::new(".")) };
        if dir.as_os_str().is_empty() {
            dir = Path::new(".");
        }
        for d in dir.ancestors() {
            let d = if d.as_os_str().is_empty() { Path::new(".") } else { d };
            if d.is_dir() {
                return d.to_path_buf();
            }
        }
        PathBuf::from(".")
    }

    let mut found: Option<(String, Home)> = None;
    for arg in paths {
        let home = home::find(&anchor(arg))?;
        match (&found, home) {
            (_, None) => {}
            (None, Some(h)) => found = Some((arg.clone(), h)),
            (Some((_, h0)), Some(h)) if h0.dir == h.dir => {} // same package — nothing to record
            (Some((first, h0)), Some(h)) => {
                return Err(format!(
                    "explicit paths span two prova packages ({first:?} is in {}, {arg:?} is in {}) \
                     — run them separately, each with its own package environment",
                    h0.dir.display(),
                    h.dir.display()
                ));
            }
        }
    }
    Ok(found.map(|(_, h)| h))
}

pub(crate) fn collect_suites(
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
        // A named FILE keeps its suite membership: a sibling `suite.lua` still wraps it (the setup
        // runs, `Scope.Suite` fixtures resolve, explicitly-named members share one suite state) —
        // selection narrows the files, never their environment. Directories group via
        // `discover_suites` exactly as before.
        let mut members: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for arg in proofs {
            for path in expand_pattern(base_dir, arg)? {
                if path.is_file() {
                    if let Some(dir) = path.parent() {
                        if dir.join("suite.lua").is_file() {
                            members.entry(dir.to_path_buf()).or_default().push(path);
                            continue;
                        }
                    }
                }
                let found = discover_suites(&path).map_err(|err| format!("{arg}: {err}"))?;
                suites.extend(found);
            }
        }
        for (dir, mut files) in members {
            files.sort();
            files.dedup();
            suites.push(Suite {
                name: dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("suite")
                    .to_string(),
                setup: Some(dir.join("suite.lua")),
                files,
            });
        }
    }
    Ok(suites)
}

/// This binary's own path, for `RunConfig::with_prova_bin` — surfaced to authors as `prova.bin`.
///
/// `current_exe` is already how this CLI re-invokes itself (`prova start` spawns `prova up`), so the
/// same answer serves both and a suite driving prova recursively reaches the build that is running
/// it rather than whatever `PATH` resolves.
///
/// On the vanishingly rare failure this yields `None`, leaving `prova.bin` unset. That is deliberate:
/// guessing `"prova"` would reintroduce ambient resolution as a silent default, which is the exact
/// failure this exists to remove. A proof then fails on a nil, naming the problem.
pub(crate) fn prova_bin() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

/// Build the engine `RunConfig` every verb shares: the bundled archetect module, the global plugin
/// install root, and each resolved named plugin/namespace (so `require(...)` resolves identically
/// in `run`, `up`/`watch`, and `eval`). Callers layer verb-specific knobs (ports, snapshots,
/// selection) on top.
pub(crate) fn engine_config(
    jobs: usize,
    packages_resolved: &packages::ResolvedPackages,
    home: Option<&Home>,
    progress: std::sync::Arc<dyn prova_core::Progress>,
) -> RunConfig {
    // The ambient plugin dir is declared in the manifest (`[run] plugin_root`) — nothing global,
    // nothing from the environment, nothing from the cwd. Discovery locates `prova.toml`; from there
    // the file names everything, so a reader (or an agent) can audit what a `require` could possibly
    // resolve without knowing a single convention baked into this binary.
    let mut config = RunConfig::new(jobs)
        .with_module(prova_archetect::install)
        // Activity reporting rides RunConfig, not the reporter: it is stderr-only and ephemeral,
        // while the reporter carries durable results to stdout (see prova_core::progress).
        .with_progress(progress);
    if let Some(root) = &packages_resolved.search_root {
        config = config.with_package_root(root.clone());
    }
    // Surface where the package is (`prova.root` / `prova.home`) so repo-local plugins can find
    // repo artifacts. Absent when there is no manifest.
    if let Some(h) = home {
        config = config.with_project(h.dir.clone());
    }
    // Surface the running binary (`prova.bin`) so a suite that drives prova recursively names the
    // build under test. Unlike `prova.root` this is available with or without a manifest — it
    // describes the process, not the package.
    if let Some(bin) = prova_bin() {
        config = config.with_prova_bin(bin);
    }
    for (name, path) in &packages_resolved.named {
        config = config.with_named_package(name.clone(), path.clone());
    }
    for (canonical, dir) in &packages_resolved.namespaces {
        config = config.with_package_namespace(canonical.clone(), dir.clone());
    }
    // Each plugin's `library/*.lua` stubs feed `prova.help()` — the plugin documents itself once
    // and the IDE, help(), and MCP introspect all answer from the same files.
    for root in packages_resolved.roots.values() {
        config = config.with_help_root(root.clone());
    }
    config
}

/// Resolve ad-hoc `-P name=source` entries the same way manifest dependencies resolve and layer
/// them over `packages_resolved` (CLI wins over a manifest plugin of the same name).
pub(crate) fn layer_cli_packages(
    cli_packages: &[String],
    layout: &dyn SystemLayout,
    sources: &BTreeMap<String, String>,
    packages_resolved: &mut packages::ResolvedPackages,
) -> Result<(), ExitCode> {
    if cli_packages.is_empty() {
        return Ok(());
    }
    let mut adhoc: BTreeMap<String, manifest::PackageSource> = BTreeMap::new();
    for entry in cli_packages {
        match entry.split_once('=') {
            Some((name, source)) if !name.is_empty() && !source.is_empty() => {
                adhoc.insert(
                    name.to_string(),
                    manifest::PackageSource::Path(source.to_string()),
                );
            }
            _ => {
                eprintln!("prova: -P expects name=source, got {entry:?}");
                return Err(ExitCode::from(2));
            }
        }
    }
    // Ad-hoc `--plugin` entries are always local paths (never git), so the git freshness policy is
    // irrelevant here — a default is fine.
    match packages::resolve_packages(
        &adhoc,
        Path::new("."),
        layout,
        sources,
        PROVA_VERSION,
        &packages::GitFetchOptions::default(),
    ) {
        Ok(resolved) => {
            packages_resolved.named.extend(resolved.named);
            packages_resolved.namespaces.extend(resolved.namespaces);
            packages_resolved.roots.extend(resolved.roots);
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
pub(crate) fn resolve_from_manifest(
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
    // Whether the caller consumes `r.proofs` as its selection. A manifest run needs the key (an
    // empty selection is a config error); explicit-path runs and `eval` bring their own selection
    // and only borrow the package environment, so a plugins-only manifest is fine for them.
    require_proofs: bool,
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
    if require_proofs && resolved.proofs.is_empty() && resolved.suites.is_empty() {
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
    let git_opts = packages::GitFetchOptions {
        force: force_update || resolved.updates.force(),
        offline,
        interval: resolved.updates.interval_duration().map_err(|e| {
            eprintln!("prova: {e}");
            ExitCode::from(2)
        })?,
    };

    // Resolve declared plugins relative to the home directory (git sources fetched into cache).
    let mut packages_resolved = packages::resolve_packages(
        &resolved.dependencies,
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
    packages::prune_package_cache(layout, retention);

    // The declared plugin dir, absolutised against the package ROOT (like `paths`, and unlike the
    // home-relative `config`). Nothing is added here: a package scans exactly the one directory its
    // manifest names, so the file answers "where can a plugin come from?" on its own.
    packages_resolved.search_root = resolved.packages_dir.as_ref().map(|r| home.dir.join(r));

    // The other half of the reserved-name registry (api-freeze §2): a plugin-ROOT file bearing a
    // bundled namespace name would shadow it for every `require` — the same silent collision the
    // `[plugins]` check refuses, so it is the same manifest validation error. Checked here, where
    // the root is known, before anything runs.
    if let Some(root) = &packages_resolved.search_root {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = if path.is_dir() {
                    path.file_name().and_then(|s| s.to_str()).map(String::from)
                } else if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                    path.file_stem().and_then(|s| s.to_str()).map(String::from)
                } else {
                    None
                };
                if let Some(name) = name {
                    if prova_core::RESERVED_NAMESPACES.contains(&name.as_str()) {
                        eprintln!(
                            "prova: packages directory {}: `{name}` is a reserved prova namespace — a \
                             package cannot shadow a bundled global; rename it",
                            root.display()
                        );
                        return Err(ExitCode::from(2));
                    }
                }
            }
        }
    }

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
            &engine_config(1, &packages_resolved, Some(home), prova_core::progress::null()),
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
    let progress = match resolved.progress.as_deref() {
        None => None,
        Some(s) => match progress::Mode::parse(s) {
            Ok(mode) => Some(mode),
            Err(e) => {
                eprintln!("prova: {e} (in manifest)");
                return Err(ExitCode::from(2));
            }
        },
    };
    let github = match resolved.github.as_deref() {
        None => None,
        Some(s) => match report::GhaMode::parse(s) {
            Some(mode) => Some(mode),
            None => {
                eprintln!("prova: unknown github {s:?} in manifest (expected auto|on|off)");
                return Err(ExitCode::from(2));
            }
        },
    };
    Ok(ManifestRun {
        proofs: resolved.proofs,
        jobs,
        format,
        color,
        progress,
        quiet: resolved.quiet,
        github,
        junit: resolved.junit,
        suites: resolved.suites,
        dependencies: packages_resolved,
        sources: resolved.sources,
        manage,
        topologies: resolved.topologies,
        capabilities,
        globals_inject: resolved.globals_inject,
        placement_broker: manifest.placement.as_ref().and_then(|p| p.broker.clone()),
        heed: resolved.heed,
        lane_tags: resolved.lane_tags,
        switches: resolved.switches,
    })
}
