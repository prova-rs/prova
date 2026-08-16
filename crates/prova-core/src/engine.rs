//! The engine: inject `prova`, collect a node tree + fixture registry, then execute asynchronously.
//!
//! Async is foundational (bodies driven via `call_async`; many run concurrently on one Lua state).
//! This increment adds the **fixture / scope / teardown machine**:
//!   - `prova.fixture(name, scope, factory)` → a typed handle; `ctx:use(handle)` builds-or-caches.
//!   - Scopes `test`/`file`/`suite` with per-scope caches; a fixture is built lazily on first use.
//!   - `ctx:defer(fn)` registers LIFO teardown in the fixture's *own* scope; scopes tear down
//!     inner→outer (test before file before suite), so dependencies outlive dependents.
//!   - `ctx:tempdir()` — scratch dir auto-removed at scope end.
//!
//! It also adds **flows** (`prova.flow` / `g:flow`, `f:step`): a flow is one scheduling unit
//! (`PlanUnit::Flow`) whose steps run serially in declared order, sharing closure upvalues and a
//! `flow`-scope instance; once a step fails the rest cascade-skip. Flows parallelize with sibling
//! units like any other unit.
//!
//! And the **dependency DAG** (`depends_on`): `prova.test`/`flow`/`group` return `UnitHandle`s;
//! `build_plan` flattens the tree to `Leaf`s (tests + flows) and expands each unit's `depends_on`
//! (folding in inherited group-level deps) into concrete leaf edges. The scheduler (`run_plan`)
//! runs a leaf only once all its dependency leaves have **passed**; if any failed or was skipped it
//! cascade-skips (transitively). Independent leaves run concurrently up to `concurrency`; an edge
//! orders and gates regardless of the job count — so this is the substrate for safe parallelism.
//!
//! And **resources** (`prova.port`/`writes`/`reads`, `serial`): each leaf carries `reqs`, and a
//! readers-writer `ResourceTable` gates launches so a writer excludes all holders of a token while
//! readers overlap. Acquisition is all-or-nothing per leaf (no hold-and-wait → no deadlock);
//! `serial` desugars to an exclusive hold on a reserved global token every other leaf reads.
//! Declarations are inert at `concurrency = 1` and enforced above it.
//!
//! Execution defaults to **sequential** (`concurrency = 1`): correct and deterministic for
//! fixture-sharing tests. Parallelism is opt-in via `RunConfig`/`--jobs`, made safe by the resource
//! scheduler. **`ctx:use` is an async method**, so fixture factories can `await` (e.g. `shell.run`);
//! the capability modules (`shell`, `fs`) live in `modules.rs`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::stream::StreamExt;
use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::model::{
    parse_duration, Event, NodeIx, Outcome, Params, ReminderAccount, ReminderListing,
    ReminderOutcome, ReminderState, Reporter, ResourceReq, Summary, UnitOpts,
};

/// Throughput knob (never semantic). Defaults to sequential until the resource scheduler exists.
/// A plugin module: registers extra globals (e.g. an `archetect` table) into a fresh Lua state.
/// Called once per state, on worker threads, so it must be `Send + Sync`. Built-in modules
/// (`shell`, `fs`) are always installed; these are added by the host (CLI / an integration crate),
/// keeping `prova-core` domain-agnostic.
pub type Module = std::sync::Arc<dyn Fn(&Lua) -> mlua::Result<()> + Send + Sync>;

/// Which collected nodes a run executes. Empty = everything. Composable, agent-friendly
/// selection: `keywords` are case-insensitive substrings of the full node path (`-k`), `tags`
/// match a leaf's effective tags (own + inherited from enclosing groups; `--tags`), `nodes` are
/// exact node paths (`--node` — re-run precisely the node a report named). `*_excludes` remove
/// after the includes select. Dependencies of selected leaves are ALWAYS pulled in: an outcome
/// gate can't be evaluated against a node that never ran.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub keywords: Vec<String>,
    pub keyword_excludes: Vec<String>,
    pub tags: Vec<String>,
    pub tag_excludes: Vec<String>,
    pub nodes: Vec<String>,
    /// Claim addresses whose covering proofs to select (`--covering`, repeatable): a full
    /// address (`docs/x.md#id`), a bare id, or a whole doc path — the three grains a brief names
    /// a gate at (docs/design/agent-ergonomics.md#claim-scoped-selection). A leaf is selected
    /// when any of its `covers` bindings (pin-stripped) discharges any listed target.
    pub covering: Vec<String>,
    /// The lane's baked tag selection (`[profiles.<name>] tags`, split on `!` like `--tags`).
    /// An INDEPENDENT gate ANDed with the CLI axes above: the lane defines the set, the CLI
    /// narrows within it — never widens past it.
    pub lane_tags: Vec<String>,
    pub lane_tag_excludes: Vec<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
            && self.keyword_excludes.is_empty()
            && self.tags.is_empty()
            && self.tag_excludes.is_empty()
            && self.nodes.is_empty()
            && self.covering.is_empty()
            && self.lane_tags.is_empty()
            && self.lane_tag_excludes.is_empty()
    }

    /// Does a reminder survive this selection? The ONE reminder matcher — the report narrows
    /// declared rows through it and heed gates recorded entries through it, so the grammar
    /// cannot drift between reading the lane and gating on it
    /// (docs/design/reminders.md#heed-selector-is-the-one-grammar). The paths a reminder
    /// answers to: its name (the exact `--node` address, a `-k` substring) and its declaring
    /// file when known (recorded entries carry it; a bare declaration may not).
    pub fn selects_reminder(&self, name: &str, file: Option<&str>, tags: &[String]) -> bool {
        let mut paths: Vec<&str> = vec![name];
        if let Some(f) = file {
            paths.push(f);
        }
        // A reminder covers nothing, so a `--covering` selection never matches one — the axis
        // addresses the proof lane by construction.
        self.selects(&paths, tags, &[])
    }

    /// Does a `covers` binding (pin-stripped) discharge one of the `--covering` targets? A
    /// target matches as the full address, the bare `#id`, or the whole doc path.
    fn covering_matches(target: &str, covers: &[String]) -> bool {
        covers.iter().any(|c| {
            let (addr, _pin) = crate::ledger::split_pin(c);
            addr == target
                || addr
                    .split_once('#')
                    .is_some_and(|(doc, id)| id == target || doc == target)
        })
    }

    /// Does a leaf with these paths, effective tags, and `covers` bindings survive this selection?
    fn selects(&self, paths: &[&str], tags: &[String], covers: &[String]) -> bool {
        // The lane gate first, independent of the CLI axes: a leaf outside the lane's tag set is
        // out regardless of what -k/--node/--tags would say — the CLI narrows, never escapes.
        if !self.lane_tags.is_empty() && !self.lane_tags.iter().any(|t| tags.contains(t)) {
            return false;
        }
        if self.lane_tag_excludes.iter().any(|t| tags.contains(t)) {
            return false;
        }
        let lower: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
        // Includes: with no include criteria at all, everything is a candidate.
        let has_includes = !self.keywords.is_empty()
            || !self.nodes.is_empty()
            || !self.tags.is_empty()
            || !self.covering.is_empty();
        let mut included = !has_includes;
        if !included && !self.keywords.is_empty() {
            included = self
                .keywords
                .iter()
                .any(|k| lower.iter().any(|p| p.contains(&k.to_lowercase())));
        }
        if !included && !self.nodes.is_empty() {
            included = self.nodes.iter().any(|n| paths.contains(&n.as_str()));
        }
        if !included && !self.tags.is_empty() {
            included = self.tags.iter().any(|t| tags.contains(t));
        }
        if !included && !self.covering.is_empty() {
            included = self.covering.iter().any(|t| Self::covering_matches(t, covers));
        }
        // When multiple include axes are given, any-axis match includes (they compose as OR) —
        // excludes below are what narrow.
        if !included {
            return false;
        }
        if self
            .keyword_excludes
            .iter()
            .any(|k| lower.iter().any(|p| p.contains(&k.to_lowercase())))
        {
            return false;
        }
        if self.tag_excludes.iter().any(|t| tags.contains(t)) {
            return false;
        }
        true
    }
}

/// How published container ports bind to the host. Tests always want `Auto` (a random host port per
/// container, so parallel runs never collide); an *inhabited* topology (`prova up --fixed`) can ask
/// for `Fixed`, pinning each published port to its canonical container port so external tools connect
/// on a predictable address and advertised-listener resources (Kafka) can compute their listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PortMode {
    /// Random host port per published container port (the testing default).
    #[default]
    Auto,
    /// Pin each published port to its canonical container port on the host.
    Fixed,
}

impl PortMode {
    /// The value exposed to Lua as `prova.ports` (`"auto"` | `"fixed"`), which `prova.containerized`
    /// reads to decide whether to upgrade plain (random) ports to fixed bindings.
    fn as_str(self) -> &'static str {
        match self {
            PortMode::Auto => "auto",
            PortMode::Fixed => "fixed",
        }
    }
}

/// A thread-safe set of every `.snap` file referenced during a run — shared across worker Lua states
/// so the CLI can find untouched (orphaned) snapshots afterward.
pub type SnapshotRegistry = std::sync::Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>;

/// Find orphaned `.snap` files after a run: those present on disk in a `snapshots/` dir that a test
/// *did* reference, but which were not themselves referenced. Only dirs with at least one referenced
/// snapshot are scanned — so a fully-deselected test file's snapshots are never examined (no false
/// positives from selection). Returns sorted paths. Sound only on a full run; the caller gates on that.
pub fn unreferenced_snapshots(registry: &SnapshotRegistry) -> Vec<PathBuf> {
    let touched = match registry.lock() {
        Ok(set) => set.clone(),
        Err(_) => return Vec::new(),
    };
    let mut dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for snap in &touched {
        if let Some(dir) = snap.parent() {
            dirs.insert(dir.to_path_buf());
        }
    }
    let mut orphans = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("snap") && !touched.contains(&p) {
                orphans.push(p);
            }
        }
    }
    orphans.sort();
    orphans
}

#[derive(Clone)]
pub struct RunConfig {
    pub concurrency: usize,
    /// Node selection applied after collection (empty = run everything).
    pub selection: Selection,
    /// Host port binding strategy. `Auto` for tests; `Fixed` only when an inhabited topology is stood
    /// up with `--fixed`.
    pub ports: PortMode,
    /// When set (`--update-snapshots`), `matches_snapshot` (re)writes `.snap` files and passes,
    /// instead of comparing against them.
    pub update_snapshots: bool,
    /// If present, every `.snap` a `matches_snapshot` references is recorded here (shared across
    /// workers), so the caller can reconcile untouched snapshots (`--unreferenced`) after a full run.
    snapshot_registry: Option<SnapshotRegistry>,
    /// If present, every deputed case a verifier facet ingests (`junit.verify`) is recorded here
    /// (shared across workers), so the caller can file the deputed account into the run record.
    deputed_registry: Option<crate::model::DeputedRegistry>,
    /// If present, every measurement a `measure.record`/`measure.ratchet` call takes is recorded
    /// here (shared across workers), so the caller can file them into the run record and the
    /// baseline writer (`--update-baseline`) can read this run's observed values.
    measurement_registry: Option<crate::model::MeasurementRegistry>,
    /// If present, every `report.publish` lands here — the artifacts a conduct produced, for the
    /// caller to file into the run record (docs/design/verifiers.md#reports-are-custody-not-visualization).
    report_registry: Option<crate::model::ReportRegistry>,
    /// Where published artifacts are COPIED so they outlive the conduct that made them
    /// (`<home>/.prova/var/reports`). Absent without a package: there is nowhere durable to put
    /// them, so the report names where the artifact already lies.
    report_custody: Option<std::path::PathBuf>,
    modules: Vec<Module>,
    /// Extra disk roots the plugin searcher consults (e.g. the global `data_dir/plugins`).
    package_roots: Vec<std::path::PathBuf>,
    /// Manifest-declared plugins: name → an exact file (a local path, or a git checkout the CLI
    /// fetched into the cache). Authoritative over disk roots.
    named_packages: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Plugin namespaces: a plugin's canonical name → its module root dir, so a multi-file plugin can
    /// `require("<canonical>.<sub>")` its own siblings.
    package_namespaces: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Resolved plugin roots whose `library/*.lua` stubs feed `prova.help()` (see `with_help_root`).
    help_roots: Vec<std::path::PathBuf>,
    /// The project ROOT — the base every manifest-relative path resolves against (for a nested
    /// manifest, the dir ABOVE the `prova/`/`.prova/` nook, not where the manifest file sits).
    /// Surfaced to authors as `prova.root` / `prova.home` (synonyms). See `with_project`.
    project_dir: Option<std::path::PathBuf>,
    /// Manifest-declared topologies (`[topologies]`): each desugars to a `prova.topology(alias,
    /// require(plugin).factory)` call the up/list path execs after loading files. Empty for a plain
    /// run — these only matter to the `up`/`watch`/list verbs.
    topology_registrations: Vec<TopologyRegistration>,
    /// Capabilities the project's `prova.lua` companion registered — per run, so two projects
    /// resolved in one process don't share a vocabulary. Empty when there is no companion; built-in
    /// capabilities (`docker`, `unix`, tools on PATH) work regardless.
    capabilities: Capabilities,
    /// The run-wide `Scope.Run` conduct store (docs/plans/shared-deputies.md) — minted once per
    /// config, cloned into every suite's `RunState`, so one conduct feeds every worker.
    pub(crate) conducts: ConductRegistry,
    /// `--due` (driver mode): open promises report as real failures — the implementing
    /// agent's loop sees full red. The graduate-on-pass inversion applies in both modes.
    pub due: bool,
    /// `--specs` (the selector): narrow the run to leaves carrying an effective spec flag —
    /// graduated leaves and ordinary tests are deselected. Composes with `--list`.
    pub promises_only: bool,
    /// `--proofs` (the mirror selector): narrow to leaves that are NOT promised — the settled,
    /// implemented proofs. The state complement of `promises_only`; the two are mutually exclusive
    /// (the CLI rejects both). Composes with `--list` (the `prova tests --proofs` view).
    pub proofs_only: bool,
    /// Run the falsification pass: select only leaves declaring `falsified_by`, apply the mutation
    /// before the body, and invert the verdict — a body that survives is vacuous.
    pub falsify: bool,
    /// The thrown opt-in switches (`-s`, `[run]`/profile `switches` — union across all doors). A
    /// leaf carrying a `switch` not in this set is held back from the run, deselected-not-skipped
    /// (docs/design/manifest.md#switches-not-env-capabilities).
    pub switches: std::collections::BTreeSet<String>,
    /// Where activity during a blocking pause is reported (see [`crate::progress`]). Deliberately
    /// here rather than on the reporter: activity is stderr-only and ephemeral, while the reporter
    /// carries durable results to stdout. Defaults to a silent sink, so a library consumer or a test
    /// pays nothing.
    progress: std::sync::Arc<dyn crate::progress::Progress>,
    /// `[globals] inject = [...]`: the module names (bundled and/or plugin) bound as unqualified
    /// ambient globals. Non-injected modules stay reachable as `prova.<name>` / `require(name)` — they
    /// are simply not ambient. The core authoring globals `prova`/`Scope` are always injected on top.
    globals_inject: Vec<String>,
    /// The prova executable driving this run, surfaced to authors as `prova.bin` (see
    /// `with_prova_bin`). Injected rather than read from `std::env::current_exe()` here: prova-core
    /// is a library, and a suite embedding it must not have "the current process is prova" assumed
    /// on its behalf. `None` when the embedder does not supply one — `prova.bin` is then absent, and
    /// a proof that needs it fails saying so rather than silently falling back to `PATH`.
    prova_bin: Option<std::path::PathBuf>,
    /// Held topologies offered to this run (docs/design/topologies.md#attach-binds-by-name): each
    /// entry is a running holder's recorded value snapshot. When the collection declares a topology
    /// of the same name, the snapshot is rehydrated into the scope caches instead of running the
    /// factory — the cross-process sibling of the MCP warm path's same-Lua value injection.
    attached: Vec<AttachedTopology>,
    /// If present, every attached topology name that actually BOUND this run is recorded here
    /// (the deputed-registry pattern), so the caller can announce it and file it into the run
    /// record as provenance: an attached run's evidence is live-state, not hermetic.
    attached_registry: Option<AttachedRegistry>,
}

/// A held topology a run may attach to: its name plus the holder's recorded JSON projection of
/// the factory's returned value. Only JSON-representable structure survives the projection —
/// closures and userdata do not, and must not: the resource grammar says clients attach by `url`.
#[derive(Clone, Debug)]
pub struct AttachedTopology {
    pub name: String,
    pub value: serde_json::Value,
}

/// Names of held topologies that actually bound a run — shared with the caller, the same shape as
/// the deputed/measurement registries.
pub type AttachedRegistry = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// One `Scope.Run` fixture's run-wide slot (docs/plans/shared-deputies.md): claimed before the
/// factory runs, settled to its outcome after. `Conducting` is what a second worker blocks on;
/// both settled states memoize for the rest of the run — failure exactly as success
/// (docs/design/lifecycle.md#fixture-failure-memoization), now across suites.
pub(crate) enum ConductSlot {
    Conducting,
    Ready(serde_json::Value),
    Poisoned(String),
}

/// The run-wide conduct store, keyed by fixture NAME (the run-level contract — Lua states have
/// their own ids). Shared across workers exactly as the snapshot/deputed/measurement registries
/// are: one `Arc` minted per `RunConfig`, cloned into every suite's `RunState`.
pub(crate) type ConductRegistry =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, ConductSlot>>>;

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            concurrency: 1,
            selection: Selection::default(),
            ports: PortMode::default(),
            update_snapshots: false,
            snapshot_registry: None,
            deputed_registry: None,
            measurement_registry: None,
            report_registry: None,
            report_custody: None,
            modules: Vec::new(),
            package_roots: Vec::new(),
            named_packages: std::collections::BTreeMap::new(),
            package_namespaces: std::collections::BTreeMap::new(),
            help_roots: Vec::new(),
            project_dir: None,
            topology_registrations: Vec::new(),
            capabilities: Capabilities::default(),
            due: false,
            promises_only: false,
            proofs_only: false,
            falsify: false,
            switches: std::collections::BTreeSet::new(),
            conducts: ConductRegistry::default(),
            progress: std::sync::Arc::new(crate::progress::NullProgress),
            // Default to injecting the full bundled set, so any RunConfig that does not customize
            // injection (eval, up, watch) still exposes the ambient globals — matching the old
            // "empty exclude = inject all" default. The manifest-run path overrides this with the
            // package's resolved `[globals] inject` list.
            globals_inject: crate::default_inject(),
            prova_bin: None,
            attached: Vec::new(),
            attached_registry: None,
        }
    }
}

impl std::fmt::Debug for RunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunConfig")
            .field("concurrency", &self.concurrency)
            .field("selection", &self.selection)
            .field("ports", &self.ports)
            .field("update_snapshots", &self.update_snapshots)
            .field("modules", &self.modules.len())
            .field("package_roots", &self.package_roots)
            .field("named_packages", &self.named_packages)
            .field("package_namespaces", &self.package_namespaces)
            .finish()
    }
}

impl RunConfig {
    pub fn new(concurrency: usize) -> Self {
        RunConfig {
            concurrency,
            ..Default::default()
        }
    }

    /// Attach the project's registered capabilities (from `prova.lua`), so `requires` resolution
    /// during the run sees the same vocabulary the `must_run` precondition did.
    pub fn with_capabilities(mut self, caps: Capabilities) -> Self {
        self.capabilities = caps;
        self
    }

    /// Install the sink that reports activity during blocking pauses (see [`crate::progress`]).
    /// Without this a run is silent through a pull or a readiness poll, which is the default for
    /// library consumers and every test — only the CLI installs a real renderer.
    pub fn with_progress(mut self, progress: std::sync::Arc<dyn crate::progress::Progress>) -> Self {
        self.progress = progress;
        self
    }

    /// The activity sink, for the module layer to bracket its blocking regions with.
    pub(crate) fn progress(&self) -> &std::sync::Arc<dyn crate::progress::Progress> {
        &self.progress
    }

    /// Module names to bind as unqualified ambient globals (`[globals] inject`). The CLI validates the
    /// names (bundled module or declared plugin); the engine just honors the list. `prova`/`Scope` are
    /// always injected regardless.
    pub fn with_globals_inject(mut self, inject: Vec<String>) -> Self {
        self.globals_inject = inject;
        self
    }

    /// Set the host port binding strategy (`Auto` for tests, `Fixed` for an inhabited topology stood
    /// up with `--fixed`).
    pub fn with_ports(mut self, ports: PortMode) -> Self {
        self.ports = ports;
        self
    }

    /// Enable snapshot-update mode (`--update-snapshots`): `matches_snapshot` (re)writes `.snap` files.
    pub fn with_update_snapshots(mut self, update: bool) -> Self {
        self.update_snapshots = update;
        self
    }

    /// `--due` (driver mode): open promises report as real failures.
    pub fn with_due(mut self, strict: bool) -> Self {
        self.due = strict;
        self
    }

    /// `--specs` (the selector): run only the leaves carrying an effective spec flag.
    pub fn with_promises_only(mut self, promises_only: bool) -> Self {
        self.promises_only = promises_only;
        self
    }

    /// `--proofs` (the mirror selector): run only the settled leaves — those NOT promised.
    pub fn with_proofs_only(mut self, proofs_only: bool) -> Self {
        self.proofs_only = proofs_only;
        self
    }

    pub fn with_falsify(mut self, falsify: bool) -> Self {
        self.falsify = falsify;
        self
    }

    /// Throw opt-in switches for this run (union across the doors: CLI `-s`, `[run]`/profile
    /// `switches`). Leaves gated on an unthrown switch are held back, deselected-not-skipped.
    pub fn with_switches<I: IntoIterator<Item = String>>(mut self, switches: I) -> Self {
        self.switches.extend(switches);
        self
    }

    /// Record every referenced `.snap` into `registry`, so the caller can reconcile unreferenced
    /// snapshots after the run (`--unreferenced`).
    /// Attach the deputed-case registry — every `junit.verify`-style ingestion lands here, for
    /// the caller to file into the run record (docs/design/verifiers.md).
    pub fn with_deputed_tracking(mut self, registry: crate::model::DeputedRegistry) -> Self {
        self.deputed_registry = Some(registry);
        self
    }

    /// Attach the measurement registry — every `measure.record`/`measure.ratchet` call lands here,
    /// for the caller to file into the run record and to feed the guarded `--update-baseline` writer.
    pub fn with_measurement_tracking(mut self, registry: crate::model::MeasurementRegistry) -> Self {
        self.measurement_registry = Some(registry);
        self
    }

    /// Attach the report registry and the custody root — every `report.publish` lands in the
    /// former, and its artifact is copied under the latter so it survives `target/` being swept.
    pub fn with_report_tracking(
        mut self,
        registry: crate::model::ReportRegistry,
        custody: Option<std::path::PathBuf>,
    ) -> Self {
        self.report_registry = Some(registry);
        self.report_custody = custody;
        self
    }

    pub fn with_snapshot_tracking(mut self, registry: SnapshotRegistry) -> Self {
        self.snapshot_registry = Some(registry);
        self
    }

    /// Register a plugin module — a `Fn(&Lua) -> Result<()>` run against every Lua state the run
    /// creates. Use this to inject domain globals (e.g. `prova_archetect::install`).
    pub fn with_module<F>(mut self, install: F) -> Self
    where
        F: Fn(&Lua) -> mlua::Result<()> + Send + Sync + 'static,
    {
        self.modules.push(std::sync::Arc::new(install));
        self
    }

    /// Add a disk root the plugin searcher consults, beyond the project's own `.prova/plugins`
    /// (which `with_project` already implies). An embedder's extension point — the CLI passes
    /// nothing here on purpose, so a run resolves only what the project has under version control.
    pub fn with_package_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.package_roots.push(root.into());
        self
    }

    /// Register a manifest-declared plugin: `require(name)` resolves to `path` (a local file or a
    /// git checkout already fetched into the cache).
    pub fn with_named_package(
        mut self,
        name: impl Into<String>,
        path: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.named_packages.insert(name.into(), path.into());
        self
    }

    /// Register a plugin namespace: `require("<canonical>.<sub>")` resolves `<sub>` under `dir`, so a
    /// multi-file plugin can require its own sibling modules.
    /// Where the project ROOT is — the base every manifest-relative path resolves against. Root and
    /// home are the same thing; for a nested `prova/` / `.prova/` manifest this is the PARENT of the
    /// nook (the nook holds prova's own files; the root stays above it).
    ///
    /// Surfaced to authors as **`prova.root`** and **`prova.home`** (synonyms). A repo-local plugin
    /// needs it to locate repo artifacts — a built binary, testdata — relative to the project
    /// (`prova.root .. "/target/debug/app"`) instead of an absolute path (unshippable) or the process
    /// cwd (an undocumented coincidence CI breaks). See `docs/design/agent-ergonomics.md` §2.
    pub fn with_project(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.project_dir = Some(dir.into());
        self
    }

    /// The prova executable driving this run, surfaced to authors as `prova.bin`.
    ///
    /// A suite that drives prova itself — prova's own proofs, an archetype proving what it renders,
    /// any project whose tool under test invokes prova recursively — must reach a *specific* binary,
    /// not whichever one `PATH` happens to resolve. Ambient resolution is not hermetic: with several
    /// checkouts sharing one `~/.cargo/bin/prova`, the nested call silently tests a different build
    /// than the one running the suite, and the failure surfaces as a proof failing on a symbol that
    /// demonstrably exists in the tree.
    ///
    /// The caller passes its own executable (`std::env::current_exe()`), which makes the nested run
    /// self-referential by construction: the suite tests the binary that is running it, with nothing
    /// to remember and no environment to arrange.
    pub fn with_prova_bin(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.prova_bin = Some(path.into());
        self
    }

    /// Offer a held topology to this run (docs/design/topologies.md#attach-binds-by-name): if the
    /// collection declares a topology of this name, `value` — the holder's recorded JSON projection
    /// — is rehydrated and seeded into the scope caches instead of running the factory. The holder
    /// keeps ownership: no teardown is registered for the injected value.
    pub fn with_attached_topology(mut self, name: impl Into<String>, value: serde_json::Value) -> Self {
        self.attached.push(AttachedTopology {
            name: name.into(),
            value,
        });
        self
    }

    /// Record which attached topologies actually bound (see `AttachedRegistry`), so the caller can
    /// announce attachment and file live-state provenance into the run record.
    pub fn with_attached_tracking(mut self, registry: AttachedRegistry) -> Self {
        self.attached_registry = Some(registry);
        self
    }

    /// Register a manifest topology: `alias` becomes a `prova.topology` addressable by name, resolving
    /// to `require(plugin).<factory>`. Consulted only by the `up`/`watch`/list verbs. `options`, when
    /// present, is a pre-serialized Lua table literal handed to the factory as its second argument
    /// (`factory(ctx, <options>)`); `None` registers it bare (`factory` itself, called with `(ctx)`).
    pub fn with_topology_registration(
        mut self,
        alias: impl Into<String>,
        plugin: impl Into<String>,
        factory: impl Into<String>,
        options: Option<String>,
    ) -> Self {
        self.topology_registrations.push(TopologyRegistration {
            alias: alias.into(),
            plugin: plugin.into(),
            factory: factory.into(),
            options,
        });
        self
    }

    pub fn with_package_namespace(
        mut self,
        canonical: impl Into<String>,
        dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.package_namespaces.insert(canonical.into(), dir.into());
        self
    }

    /// A resolved plugin's root dir, whose `library/*.lua` stubs feed `prova.help()` — the same
    /// files the IDE links, so a plugin documents itself once and both sinks answer.
    pub fn with_help_root(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.help_roots.push(dir.into());
        self
    }
}

mod fixtures;
pub(crate) use fixtures::*;

mod collect;
use collect::*;

mod context;
use context::*;

mod matchers;
pub(crate) use matchers::*;


mod setup;
use setup::*;


mod plan;
pub use plan::*;


mod capabilities;
pub use capabilities::*;


mod run;
use run::*;


// ---------------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------------

/// A file-backed chunk's name: the path with Lua's `@` file-source prefix, so error messages and
/// tracebacks render as `path:line:` (and truncation keeps the path's *tail*) instead of the
/// `[string "path…"]` string-chunk form. Matches the plugin loaders' existing convention.
fn file_chunk_name(path: &Path) -> String {
    format!("@{}", path.display())
}

fn read_and_collect(path: &Path, config: &RunConfig) -> mlua::Result<(Lua, SharedCollector)> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tests")
        .to_string();
    let (lua, col) = build_lua(stem, config)?;
    col.borrow_mut().set_file_path(0, path); // singleton file → index 0, for snapshot colocation
    col.borrow_mut().singleton_suite = true; // one ungrouped file: its own suite, its own state
    lua.load(&code).set_name(file_chunk_name(path)).exec()?;
    Ok((lua, col))
}

fn new_runtime() -> mlua::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all() // time (timeouts/sleep) + io (child-process pipes for the shell module)
        .build()
        .map_err(|e| mlua::Error::RuntimeError(format!("failed to start async runtime: {e}")))
}

/// Drive `fut` to completion on `rt`, alongside any task `spawn_local`'d from within it.
///
/// **Why a `LocalSet` and not plain `block_on`.** Test bodies are concurrent *futures* in a
/// `FuturesUnordered` (see `run_plan`), never `tokio::spawn`ed — so nothing here has ever needed to
/// be `Send`, and mlua's handles aren't (no `send` feature: a `Lua` is pinned to its thread). That
/// is fine until something must outlive the call that created it and still touch Lua: an
/// `http.mock` server is spawned by one test and answers requests *while another coroutine is
/// suspended*, so its task holds Lua handles and cannot be `tokio::spawn`ed at any price — that
/// bound is on `spawn`, not on the runtime flavor, so `rt-multi-thread` would not help either.
/// `spawn_local` is the mechanism for exactly this, and it requires a `LocalSet` to be the thing
/// being polled. `run_until` polls the local task set and `fut` together, so a mock server is
/// driven whenever a test awaits — which is the whole runtime assumption `http.mock` rests on.
///
/// Cheap and total: one allocation per run, and every `block_on` in the engine goes through here so
/// no execution path is quietly the odd one out where a local task silently never runs.
fn block_on_local<F: std::future::Future>(rt: &tokio::runtime::Runtime, fut: F) -> F::Output {
    let local = tokio::task::LocalSet::new();
    local.block_on(rt, fut)
}

pub fn run_path(path: &Path, reporter: &mut dyn Reporter) -> mlua::Result<Summary> {
    run_path_with(path, reporter, &RunConfig::default())
}

pub fn run_path_with(
    path: &Path,
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    reporter.event(&Event::RunStarted);
    let summary = run_file_into(path, reporter, config)?;
    reporter.event(&Event::RunFinished { summary: &summary });
    Ok(summary)
}

/// Run a single file end to end, emitting **only node-level events** (no `RunStarted`/`RunFinished`)
/// so a suite coordinator can own the run-level events across many files. Creates its own Lua state
/// and Tokio runtime, so it is self-contained on whatever thread (worker) calls it — the basis for
/// per-worker-Lua-state parallelism across files.
pub(crate) fn run_file_into(
    path: &Path,
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    let (lua, col) = read_and_collect(path, config)?;
    execute_collected(&lua, &col, reporter, config)
}

/// Run a **suite** — several files loaded into one Lua state so `Scope.Suite` fixtures are shared
/// live across them (built once, torn down once). An optional `setup` file (a `suite.lua`) runs first
/// and is where suite-scoped fixtures live; each member `file` then loads under its own file-group
/// (so report paths show the file and `Scope.File` is per-file). A one-file suite with no setup is
/// exactly `run_file_into` — the singleton case — so nothing changes for ungrouped files.
pub(crate) fn run_suite_files(
    name: &str,
    setup: Option<&Path>,
    files: &[PathBuf],
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    if setup.is_none() && files.len() == 1 {
        return run_file_into(&files[0], reporter, config);
    }

    let (lua, col) = build_lua(name.to_string(), config)?;

    // Setup file (fixtures only) runs at the suite level (file index 0).
    if let Some(setup) = setup {
        let code = std::fs::read_to_string(setup).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", setup.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(setup)).exec()?;
    }

    // Each member file loads under a file-group node, with its own file index (1-based).
    load_member_files(&lua, &col, files)?;

    execute_collected(&lua, &col, reporter, config)
}

/// Load each member `file` under its own file-group node with its own file index (1-based; index 0
/// is the suite/setup level). Shared by `run_suite_files` and the warm re-run path (which re-collects
/// into a *held* Lua state instead of a fresh one).
fn load_member_files(lua: &Lua, col: &SharedCollector, files: &[PathBuf]) -> mlua::Result<()> {
    for (i, file) in files.iter().enumerate() {
        let idx = i + 1;
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        {
            let mut c = col.borrow_mut();
            c.current_file = idx;
            c.set_file_path(idx, file); // for snapshot colocation beside this member file
            let fg = c.add(0, group_node(stem));
            c.parent_stack.push(fg);
        }
        let code = std::fs::read_to_string(file).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", file.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(file)).exec()?;
        {
            let mut c = col.borrow_mut();
            c.parent_stack.pop();
            c.current_file = 0;
        }
    }
    Ok(())
}

/// The attention-account pass: evaluate every `prova.remind` condition, after the proofs have
/// completed (docs/design/reminders.md).
///
/// Load one suite the way `--list` does — setup first, then every member file; bodies never
/// execute. The shared front half of every query verb that reads a collection without running it
/// (`evaluate_reminders`, `collect_reminders`, `collect_switch_census`).
fn load_suite_collection(
    suite: &crate::suite::Suite,
    config: &RunConfig,
) -> mlua::Result<(Lua, SharedCollector)> {
    if suite.setup.is_none() && suite.files.len() == 1 {
        return read_and_collect(&suite.files[0], config);
    }
    load_collection(&suite.name, suite.setup.as_deref(), &suite.files, config)
}

/// The parts-shaped core of `load_suite_collection`: a fresh state, the optional setup chunk,
/// then every member file — bodies never execute. Discovery calls this directly where it has
/// parts rather than a `Suite`.
fn load_collection(
    name: &str,
    setup: Option<&Path>,
    files: &[PathBuf],
    config: &RunConfig,
) -> mlua::Result<(Lua, SharedCollector)> {
    let (lua, col) = build_lua(name.to_string(), config)?;
    if let Some(setup) = setup {
        let code = std::fs::read_to_string(setup).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", setup.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(setup)).exec()?;
    }
    load_member_files(&lua, &col, files)?;
    Ok((lua, col))
}

/// Runs during the same invocation as the proofs — conditions evaluate in RUNS, and query verbs
/// only ever read what this recorded — but in its own phase and its own per-suite Lua states,
/// because reminder closures are `!Send` and the worker states that collected them are gone by the
/// time the whole-run account (which ledger conditions observe) is known. Re-loading is the same
/// collection `--list` performs; bodies never execute.
///
/// One pass, declaration order, no fixpoint: a condition receives the run's [`ReminderAccount`]
/// and nothing about other reminders. Best-effort per suite — a suite that fails to load already
/// failed the run itself, so the pass skips it rather than failing twice.
pub fn evaluate_reminders(
    suites: &[crate::suite::Suite],
    config: &RunConfig,
    account: &ReminderAccount,
) -> Vec<ReminderOutcome> {
    let mut out = Vec::new();
    for suite in suites {
        let Ok((lua, col)) = load_suite_collection(suite, config) else { continue };
        let (defs, file_paths) = {
            let mut c = col.borrow_mut();
            (std::mem::take(&mut c.reminders), c.file_paths.clone())
        };
        if defs.is_empty() {
            continue;
        }
        let Ok(rt) = new_runtime() else { continue };
        block_on_local(&rt, async {
            for def in defs {
                let file = file_paths
                    .get(def.file)
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.to_string_lossy().into_owned());
                let state = evaluate_reminder(&lua, &def, config, account).await;
                out.push(ReminderOutcome {
                    name: def.name,
                    message: def.message,
                    tags: def.tags,
                    file,
                    line: def.line,
                    state,
                });
            }
        });
    }
    out
}

/// Collect every declared `prova.remind` — name, message, tags, file, line — WITHOUT evaluating
/// its condition. The rows `prova reminders` shows before a run has produced states: loading is the
/// same collection `--list` performs (bodies never execute), and no condition runs, so it works with
/// no record at all. A run then fills each row's state in.
pub fn collect_reminders(suites: &[crate::suite::Suite], config: &RunConfig) -> Vec<ReminderListing> {
    let mut out = Vec::new();
    for suite in suites {
        let Ok((_lua, col)) = load_suite_collection(suite, config) else { continue };
        let (defs, file_paths) = {
            let mut c = col.borrow_mut();
            (std::mem::take(&mut c.reminders), c.file_paths.clone())
        };
        for def in defs {
            let file = file_paths
                .get(def.file)
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_string_lossy().into_owned());
            // The one selector grammar narrows this lane like every lane
            // (docs/design/reminders.md#reminders-selectors-narrow): `-k` is a substring over the
            // name and declaring file, `--node` the exact address, `--tags` the reminder's tags.
            let selected =
                config.selection.selects_reminder(&def.name, file.as_deref(), &def.tags);
            if !selected {
                continue;
            }
            out.push(ReminderListing {
                name: def.name,
                message: def.message,
                tags: def.tags,
                file,
                line: def.line,
            });
        }
    }
    out
}

/// Census the declared opt-in classes: every `switch = "<class>"` in the suite (own or inherited
/// from a group/`suite.config`), with how many leaves each class gates. Loads like `--list` —
/// bodies never execute — so `prova switches` answers before any run
/// (docs/design/manifest.md#switches-are-discoverable).
pub fn collect_switch_census(
    suites: &[crate::suite::Suite],
    config: &RunConfig,
) -> std::collections::BTreeMap<String, usize> {
    let mut census = std::collections::BTreeMap::new();
    for suite in suites {
        let Ok((_lua, col)) = load_suite_collection(suite, config) else { continue };
        let col = col.borrow();
        let Ok(plan) = build_plan(&col, &config.capabilities) else {
            continue;
        };
        for leaf in &plan.leaves {
            for class in &leaf.switches {
                *census.entry(class.clone()).or_insert(0) += 1;
            }
        }
    }
    census
}

/// The `account.specs` rows a reminder condition composes draw-down policies over: every anchor
/// as `{ address, kind, recorded?, due?, props }`. Blessed keys are flattened onto the row; the
/// full map rides as `props`, so a custom key can never shadow `address`/`kind`.
fn specs_view(lua: &Lua, specs: &[crate::model::SpecItem]) -> mlua::Result<Table> {
    let list = lua.create_table()?;
    for (i, o) in specs.iter().enumerate() {
        let row = lua.create_table()?;
        row.set("address", o.address.as_str())?;
        row.set("kind", o.kind.as_str())?;
        for key in ["recorded", "due"] {
            if let Some(v) = o.props.get(key) {
                row.set(key, v.as_str())?;
            }
        }
        let props = lua.create_table()?;
        for (k, v) in &o.props {
            props.set(k.as_str(), v.as_str())?;
        }
        row.set("props", props)?;
        list.set(i + 1, row)?;
    }
    Ok(list)
}

/// Evaluate one reminder's condition against the run's account.
///
/// The mapping is the whole contract: a falsy return is `Watching`; a truthy return is `Due`,
/// with a string return carrying the condition's own "why" (what the world did); an unmet
/// `requires` or a raise is `Unevaluated` with the reason — never `Watching`, because a watcher
/// that could not look must stay visibly disarmed.
/// Build the read-only account view a `when` condition receives — fresh per condition, so a
/// condition that mutates its copy cannot leak the mutation into a later one, and carrying NO
/// reminder state (reminders cannot observe reminders). Errors are one string; the caller turns
/// it into `Unevaluated` exactly once.
fn build_account_view(lua: &Lua, account: &ReminderAccount) -> Result<Table, String> {
    let err = |e: mlua::Error| format!("could not build the account view: {e}");
    let acct = lua.create_table().map_err(err)?;
    for (key, value) in [
        ("passed", account.passed),
        ("failed", account.failed),
        ("skipped", account.skipped),
        ("promised", account.promised),
        ("owed", account.owed),
    ] {
        acct.set(key, value).map_err(err)?;
    }
    // The timing surface (docs/design/reminders.md#duration-drift-is-attention): this run's wall
    // time, what it recorded, and what was deliberately banked — so a drift policy is a one-line
    // condition and slowness stays attention rather than an engine constant.
    acct.set("duration_ms", account.duration_ms).map_err(err)?;
    let baselines = lua.create_table().map_err(err)?;
    for (name, value) in &account.baselines {
        baselines.set(name.as_str(), *value).map_err(err)?;
    }
    acct.set("baselines", baselines).map_err(err)?;
    // The run's measurements, name -> value, so a condition can read the same scalar a ratchet
    // gates on ("this file is at 480/500").
    let measurements = lua.create_table().map_err(err)?;
    for (name, value) in &account.measurements {
        measurements.set(name.as_str(), *value).map_err(err)?;
    }
    acct.set("measurements", measurements).map_err(err)?;
    // The run's spec items, so a draw-down condition can compose over their properties --
    // `date.days_since(o.recorded)` for the sliding window, `date.past(o.due)` for a hard
    // deadline, `o.props.<key>` for the author's own vocabulary.
    let specs = specs_view(lua, &account.specs).map_err(err)?;
    acct.set("specs", specs).map_err(err)?;
    Ok(acct)
}

async fn evaluate_reminder(
    lua: &Lua,
    def: &ReminderDef,
    config: &RunConfig,
    account: &ReminderAccount,
) -> ReminderState {
    for expr in &def.requires {
        if let Some(reason) = config.capabilities.unmet_reason(expr) {
            return ReminderState::Unevaluated { reason };
        }
    }
    let acct = match build_account_view(lua, account) {
        Ok(t) => t,
        Err(reason) => return ReminderState::Unevaluated { reason },
    };
    match def.when.call_async::<Value>(acct).await {
        Err(e) => {
            let text = e.to_string();
            let first = text.lines().next().unwrap_or("error").to_string();
            ReminderState::Unevaluated {
                reason: format!("condition raised: {first}"),
            }
        }
        Ok(Value::String(s)) => {
            let why = s.to_string_lossy().to_string();
            ReminderState::Due {
                why: (!why.is_empty()).then_some(why),
            }
        }
        Ok(v) if truthy(&v) => ReminderState::Due { why: None },
        Ok(_) => ReminderState::Watching,
    }
}

/// Build plan → run → tear down (every file scope, then the suite). Shared by the single-file and
/// multi-file loaders once the collector is populated.
fn execute_collected(
    lua: &Lua,
    col: &SharedCollector,
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    let (plan, deselected, dropped, switched_off, state) = {
        let col = col.borrow();
        let plan = build_plan(&col, &config.capabilities)?;
        // Switches first: held-back classes are not part of this run's membership at all, so the
        // selection below never counts them among what `-k` deselected.
        let (plan, switch_deselected, switch_dropped, switched_off) =
            apply_switch_filter(plan, &config.switches, &config.selection);
        let (plan, mut deselected, mut dropped) = apply_selection(plan, &config.selection);
        let (plan, falsify_deselected, falsify_dropped) = apply_falsify_filter(plan, config.falsify);
        let (plan, spec_deselected, spec_dropped) =
            apply_specs_filter(plan, config.promises_only, config.proofs_only);
        deselected += switch_deselected + falsify_deselected + spec_deselected;
        dropped.extend(switch_dropped);
        dropped.extend(falsify_dropped);
        dropped.extend(spec_dropped);
        let dropped = qualify_all(dropped, &col.file_paths);
        let state = Rc::new(RunState {
            defs: col.fixtures.clone(),
            suite: Rc::new(RefCell::new(ScopeState::default())),
            files: RefCell::new(HashMap::new()),
            file_paths: col.file_paths.clone(),
            update_snapshots: config.update_snapshots,
            snapshot_registry: config.snapshot_registry.clone(),
            falsify: config.falsify,
            conducts: config.conducts.clone(),
            progress: std::sync::Arc::clone(config.progress()),
            project_dir: config.project_dir.clone(),
        });
        (plan, deselected, dropped, switched_off, state)
    };

    // Held-topology attach (docs/design/topologies.md#attach-binds-by-name): rehydrate each
    // offered holder's recorded value and seed it into the scope caches exactly the way the warm
    // path does (see `run_warm`) — keyed by NAME, re-resolved against THIS collection's fixture
    // id, seeded into the suite scope and every file scope, with no teardown registered so the
    // holder stays the one true reaper. A name the collection does not declare is not this run's
    // business and is skipped silently.
    if !config.attached.is_empty() {
        let (topologies, n_files) = {
            let c = col.borrow();
            (c.topologies.clone(), c.file_paths.len())
        };
        for att in &config.attached {
            let Some(&id) = topologies.get(&att.name) else {
                continue;
            };
            let value = json_to_lua(lua, &att.value)?;
            state.suite.borrow_mut().cache.insert(id, value.clone());
            for idx in 0..=n_files {
                state.file_scope(idx).borrow_mut().cache.insert(id, value.clone());
            }
            if let Some(reg) = &config.attached_registry {
                // Recover a poisoned lock: the registry is a plain Vec of names, valid at every step.
                reg.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(att.name.clone());
            }
        }
    }

    let rt = new_runtime()?;
    let mut summary = Summary {
        deselected,
        deselected_paths: dropped,
        switched_off,
        reminders_declared: col.borrow().reminders.len(),
        ..Summary::default()
    };
    block_on_local(&rt, async {
        let started = Instant::now();
        run_plan(lua, &plan, &state, config, reporter, &mut summary).await;
        // Scopes tear down inner→outer: every file scope, then the suite (test scopes already torn
        // down per-test). A failure in any of them is reported as its own leaf — a suite whose
        // teardown raised has leaked something, and must not be reported green.
        let mut late = teardown_file_scopes(&state).await;
        late.extend(teardown_results(
            "suite",
            teardown_scope(&state.suite).await,
            None,
            None,
        ));
        emit_finished(reporter, &mut summary, &late);
        summary.duration = started.elapsed();
    });
    Ok(summary)
}

mod eval;
pub use eval::*;


mod topology;
pub use topology::*;


mod discover;
pub use discover::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// Small subjects print verbatim — the windowing must change nothing for the common case.
    #[test]
    fn display_windowed_leaves_small_subjects_verbatim() {
        assert_eq!(display_windowed("short output", "needle"), "\"short output\"");
    }

    /// A large non-matching subject shows its edges with the middle elided — never the whole dump.
    #[test]
    fn display_windowed_elides_the_middle_of_a_large_non_match() {
        let subject = "A".repeat(3000);
        let shown = display_windowed(&subject, "needle");
        assert!(shown.len() < 700, "still dumped {} bytes", shown.len());
        assert!(shown.contains("[2520 bytes elided]"), "{shown}");
        assert!(shown.starts_with('"') && shown.ends_with('"'));
    }

    /// The never() polarity: when the needle IS present, the window centers on the match — WHERE
    /// it matched is the actionable part of that diagnostic.
    #[test]
    fn display_windowed_centers_on_the_match_when_there_is_one() {
        let subject = format!("{}NEEDLE{}", "x".repeat(2000), "y".repeat(2000));
        let shown = display_windowed(&subject, "NEEDLE");
        assert!(shown.contains("NEEDLE"), "{shown}");
        assert!(shown.len() < 700, "still dumped {} bytes", shown.len());
        assert!(shown.contains("[1760 bytes elided]"), "leading elision: {shown}");
        // Both sides elided: the window sits in the middle of the subject.
        assert_eq!(shown.matches("bytes elided").count(), 2, "{shown}");
    }

    /// Cuts never land mid-UTF-8 — a multibyte subject must not panic the diagnostic.
    #[test]
    fn display_windowed_respects_char_boundaries() {
        let subject = "é".repeat(2000); // 2 bytes per char
        let shown = display_windowed(&subject, "needle");
        assert!(shown.contains("bytes elided"), "{shown}");
    }

    #[test]
    fn slugify_makes_filesystem_safe_keys() {
        assert_eq!(slugify("orders › creates a row"), "orders-creates-a-row");
        assert_eq!(slugify("API-shape v2!"), "api-shape-v2");
        assert_eq!(slugify("  "), "snapshot"); // empty → stable fallback
    }

    #[test]
    fn snapshot_doc_round_trips_body_even_with_tricky_content() {
        // A body that itself starts with `#!` and contains a later `---` line must round-trip.
        let body = "#!/bin/sh\necho hi\n---\nnot a delimiter";
        let doc = format_snapshot("some/test / key-1", body);
        assert_eq!(snapshot_body(&doc), body);
        // A legacy doc with no header/delimiter is treated as all-body.
        assert_eq!(snapshot_body("just a value"), "just a value");
    }

    #[test]
    fn serialize_path_honors_the_level_dial() {
        let root = std::env::temp_dir().join("prova-serialize-path-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "x").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        // Directory defaults to layout: sorted relative paths.
        assert_eq!(
            serialize_path(&root, None).unwrap(),
            "Cargo.toml\nsrc/main.rs"
        );
        // Content: `=== path ===` sections.
        let content = serialize_path(&root, Some("content")).unwrap();
        assert!(content.contains("=== Cargo.toml ===\nx"), "{content}");
        assert!(
            content.contains("=== src/main.rs ===\nfn main() {}"),
            "{content}"
        );
        // A single file serializes to its content (any level).
        assert_eq!(serialize_path(&root.join("Cargo.toml"), None).unwrap(), "x");
        // layout on a file, or an unknown level, is an error.
        assert!(serialize_path(&root.join("Cargo.toml"), Some("layout")).is_err());
        assert!(serialize_path(&root, Some("bogus")).is_err());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unreferenced_snapshots_flags_only_untouched_in_touched_dirs() {
        let root = std::env::temp_dir().join("prova-unref-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("snapshots")).unwrap();
        let referenced = root.join("snapshots/t__alpha.snap");
        let orphan = root.join("snapshots/t__beta.snap");
        std::fs::write(&referenced, "a").unwrap();
        std::fs::write(&orphan, "b").unwrap();
        // A `.snap.new` and a non-snap file must be ignored.
        std::fs::write(root.join("snapshots/t__alpha.snap.new"), "x").unwrap();
        std::fs::write(root.join("snapshots/notes.txt"), "x").unwrap();

        let reg: SnapshotRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        reg.lock().unwrap().insert(referenced.clone()); // only alpha was referenced

        let orphans = unreferenced_snapshots(&reg);
        assert_eq!(
            orphans,
            vec![orphan],
            "only the untouched .snap in a touched dir"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn line_diff_marks_changed_lines_with_context() {
        let diff = line_diff("a\nb\nc", "a\nB changed\nc");
        assert_eq!(diff, "    a\n  - b\n  + B changed\n    c");
        // Pure addition at the end.
        let add = line_diff("x", "x\ny");
        assert_eq!(add, "    x\n  + y");
    }

    #[test]
    fn extract_endpoints_walks_named_resources_sorted() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        let db = lua.create_table().unwrap();
        db.set("url", "postgres://u1").unwrap();
        let app = lua.create_table().unwrap();
        app.set("url", "http://u2").unwrap();
        t.set("db", db).unwrap();
        t.set("app", app).unwrap();
        t.set("note", "not-a-resource").unwrap(); // non-table field is ignored

        let eps = extract_endpoints(&Value::Table(t), "topo");
        assert_eq!(
            eps,
            vec![
                Endpoint {
                    name: "app".into(),
                    url: "http://u2".into()
                },
                Endpoint {
                    name: "db".into(),
                    url: "postgres://u1".into()
                },
            ]
        );
    }

    #[test]
    fn extract_endpoints_reports_a_top_level_url_under_the_topology_name() {
        let lua = Lua::new();
        let single = lua.create_table().unwrap();
        single.set("url", "amqp://only").unwrap();
        let eps = extract_endpoints(&Value::Table(single), "solo");
        assert_eq!(
            eps,
            vec![Endpoint {
                name: "solo".into(),
                url: "amqp://only".into()
            }]
        );
    }

    /// The whole selection algebra in one place: lanes gate first and the CLI narrows WITHIN
    /// them; include axes compose as OR; excludes narrow after; and an empty selection admits
    /// everything.
    #[test]
    fn selection_selects_lanes_gate_and_axes_compose() {
        let empty = Selection::default();
        assert!(empty.selects(&["a › b"], &[], &[]));

        let mut sel = Selection::default();
        sel.keywords.push("orders".into());
        assert!(sel.selects(&["Orders › creates"], &[], &[]), "keywords are case-insensitive substrings");
        assert!(!sel.selects(&["billing › charges"], &[], &[]));

        sel.tags.push("slow".into());
        assert!(sel.selects(&["billing › charges"], &["slow".into()], &[]), "include axes compose as OR");

        sel.keyword_excludes.push("charges".into());
        assert!(!sel.selects(&["billing › charges"], &["slow".into()], &[]), "excludes narrow after includes");

        let mut lane = Selection::default();
        lane.lane_tags.push("ut".into());
        assert!(!lane.selects(&["anything"], &[], &[]), "outside the lane, nothing else matters");
        assert!(lane.selects(&["anything"], &["ut".into()], &[]));
        lane.keywords.push("orders".into());
        assert!(!lane.selects(&["billing"], &["ut".into()], &[]), "the CLI narrows WITHIN the lane");

        let mut lane_ex = Selection::default();
        lane_ex.lane_tag_excludes.push("soak".into());
        assert!(!lane_ex.selects(&["x"], &["soak".into()], &[]));
        assert!(lane_ex.selects(&["x"], &[], &[]));
    }
}
