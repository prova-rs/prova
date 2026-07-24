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
    parse_duration, Event, NodeIx, Outcome, Params, Reporter, ResourceReq, Summary, UnitOpts,
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
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
            && self.keyword_excludes.is_empty()
            && self.tags.is_empty()
            && self.tag_excludes.is_empty()
            && self.nodes.is_empty()
    }

    /// Does a leaf with these paths and effective tags survive this selection?
    fn selects(&self, paths: &[&str], tags: &[String]) -> bool {
        let lower: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
        // Includes: with no include criteria at all, everything is a candidate.
        let has_includes =
            !self.keywords.is_empty() || !self.nodes.is_empty() || !self.tags.is_empty();
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
    modules: Vec<Module>,
    /// Extra disk roots the plugin searcher consults (e.g. the global `data_dir/plugins`).
    plugin_roots: Vec<std::path::PathBuf>,
    /// Manifest-declared plugins: name → an exact file (a local path, or a git checkout the CLI
    /// fetched into the cache). Authoritative over disk roots.
    named_plugins: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Plugin namespaces: a plugin's canonical name → its module root dir, so a multi-file plugin can
    /// `require("<canonical>.<sub>")` its own siblings.
    plugin_namespaces: std::collections::BTreeMap<String, std::path::PathBuf>,
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
    /// `--strict-specs` (driver mode): open specs report as real failures — the implementing
    /// agent's loop sees full red. The graduate-on-pass inversion applies in both modes.
    pub strict_specs: bool,
    /// `--specs` (the selector): narrow the run to leaves carrying an effective spec flag —
    /// graduated leaves and ordinary tests are deselected. Composes with `--list`.
    pub specs_only: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            concurrency: 1,
            selection: Selection::default(),
            ports: PortMode::default(),
            update_snapshots: false,
            snapshot_registry: None,
            modules: Vec::new(),
            plugin_roots: Vec::new(),
            named_plugins: std::collections::BTreeMap::new(),
            plugin_namespaces: std::collections::BTreeMap::new(),
            help_roots: Vec::new(),
            project_dir: None,
            topology_registrations: Vec::new(),
            capabilities: Capabilities::default(),
            strict_specs: false,
            specs_only: false,
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
            .field("plugin_roots", &self.plugin_roots)
            .field("named_plugins", &self.named_plugins)
            .field("plugin_namespaces", &self.plugin_namespaces)
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

    /// `--strict-specs` (driver mode): open specs report as real failures.
    pub fn with_strict_specs(mut self, strict: bool) -> Self {
        self.strict_specs = strict;
        self
    }

    /// `--specs` (the selector): run only the leaves carrying an effective spec flag.
    pub fn with_specs_only(mut self, specs_only: bool) -> Self {
        self.specs_only = specs_only;
        self
    }

    /// Record every referenced `.snap` into `registry`, so the caller can reconcile unreferenced
    /// snapshots after the run (`--unreferenced`).
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
    pub fn with_plugin_root(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.plugin_roots.push(root.into());
        self
    }

    /// Register a manifest-declared plugin: `require(name)` resolves to `path` (a local file or a
    /// git checkout already fetched into the cache).
    pub fn with_named_plugin(
        mut self,
        name: impl Into<String>,
        path: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.named_plugins.insert(name.into(), path.into());
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

    pub fn with_plugin_namespace(
        mut self,
        canonical: impl Into<String>,
        dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.plugin_namespaces.insert(canonical.into(), dir.into());
        self
    }

    /// A resolved plugin's root dir, whose `library/*.lua` stubs feed `prova.help()` — the same
    /// files the IDE links, so a plugin documents itself once and both sinks answer.
    pub fn with_help_root(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.help_roots.push(dir.into());
        self
    }
}

// ---------------------------------------------------------------------------------------------
// Scopes & fixtures
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Test,
    Flow,
    File,
    Suite,
}

impl ScopeKind {
    fn rank(self) -> u8 {
        match self {
            ScopeKind::Test => 0,
            ScopeKind::Flow => 1,
            ScopeKind::File => 2,
            ScopeKind::Suite => 3,
        }
    }
    fn label(self) -> &'static str {
        match self {
            ScopeKind::Test => "test",
            ScopeKind::Flow => "flow",
            ScopeKind::File => "file",
            ScopeKind::Suite => "suite",
        }
    }
}

/// A typed fixture-scope value — the members of the `Scope` global (`Scope.Test`/`Scope.Flow`/
/// `Scope.File`/`Scope.Suite`). This is the only way to name a scope; discoverable and typo-safe.
#[derive(Clone, Copy)]
struct ScopeRef {
    kind: ScopeKind,
}

impl UserData for ScopeRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("scope", |_, this| Ok(this.kind.label()));
    }
}

/// Build the `Scope` global — the typed scope constants.
fn make_scope_global(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Test",
        ScopeRef {
            kind: ScopeKind::Test,
        },
    )?;
    t.set(
        "Flow",
        ScopeRef {
            kind: ScopeKind::Flow,
        },
    )?;
    t.set(
        "File",
        ScopeRef {
            kind: ScopeKind::File,
        },
    )?;
    t.set(
        "Suite",
        ScopeRef {
            kind: ScopeKind::Suite,
        },
    )?;
    Ok(t)
}

fn parse_scope(v: Value) -> mlua::Result<ScopeKind> {
    match v {
        Value::UserData(ud) => ud.borrow::<ScopeRef>().map(|r| r.kind).map_err(|_| {
            mlua::Error::RuntimeError(
                "fixture scope must be a Scope value: Scope.Test / Scope.Flow / Scope.File / Scope.Suite"
                    .into(),
            )
        }),
        _ => Err(mlua::Error::RuntimeError(
            "fixture scope must be a Scope value: Scope.Test / Scope.Flow / Scope.File / Scope.Suite"
                .into(),
        )),
    }
}

#[derive(Clone)]
struct FixtureDef {
    name: String,
    scope: ScopeKind,
    factory: Function,
    /// True when this fixture was declared via `prova.topology` (rather than `prova.fixture`). A
    /// topology's factory context is "topology-capable": it exposes an ambient managed network on
    /// `ctx.network`. Ordinary fixtures leave it `false`, so `ctx.network` is nil for them.
    is_topology: bool,
}

/// Opaque handle returned by `prova.fixture`; carries the registry id `ctx:use` resolves.
struct FixtureHandle {
    id: usize,
}
impl UserData for FixtureHandle {}

/// Opaque handle returned by `prova.test`/`flow`/`group` (and the builder variants); carries the
/// unit's arena index so `depends_on = { handle }` can resolve the edge. Treat as opaque.
#[derive(Clone, Copy)]
struct UnitHandle {
    ix: NodeIx,
}
impl UserData for UnitHandle {}

/// A typed resource reference from `prova.port`/`writes`/`reads`. Preferred over magic-format
/// strings (`"port:8080"`) — a constructor validates and can't be typo'd into a wrong-but-valid
/// token. A bare string in a `resources` list is accepted too and is exclusive by default.
#[derive(Clone)]
struct ResourceRef {
    token: String,
    shared: bool,
}
impl UserData for ResourceRef {}

/// Live state for one scope instance: cached fixture values, LIFO teardowns, temp dirs.
#[derive(Default)]
struct ScopeState {
    cache: HashMap<usize, Value>,
    teardowns: Vec<Function>,
    tempdirs: Vec<PathBuf>,
    /// The topology's ambient managed network (a `docker.network` handle), created lazily on the
    /// first `ctx.network` access inside a topology factory and cached here on the topology's own
    /// scope instance so repeated reads return the same handle. Its teardown is registered on this
    /// same scope right after creation, so LIFO order reaps it *after* the containers joined to it.
    network: Option<Value>,
}

/// Shared across the whole suite run: the fixture registry, the one suite-scope instance, and a lazy
/// **per-file** scope instance (a suite may load several files into one state, and each gets its own
/// `Scope.File`). A single file just has one entry (index 0).
struct RunState {
    defs: Vec<FixtureDef>,
    suite: Rc<RefCell<ScopeState>>,
    files: RefCell<HashMap<usize, Rc<RefCell<ScopeState>>>>,
    /// Source path per file index (from the collector), so a test's snapshot assertion can place its
    /// `.snap` beside the file it ran from. Empty for the topology (`up`/`watch`) paths.
    file_paths: Vec<PathBuf>,
    /// When set, `matches_snapshot` writes/overwrites snapshots instead of comparing (`--update-snapshots`).
    update_snapshots: bool,
    /// Shared registry of referenced `.snap` files, for unreferenced-snapshot reconciliation.
    snapshot_registry: Option<SnapshotRegistry>,
}

impl RunState {
    /// The directory a test's `.snap` files live in: `<source-file-dir>/snapshots`, or `None` if the
    /// file index has no recorded path (e.g. an ad-hoc topology run).
    /// The source path for a file index as a display string, or `None` when the index has no
    /// recorded path (an `eval`/topology run) — feeds the reported per-leaf source location.
    fn file_path_str(&self, file: usize) -> Option<String> {
        self.file_paths
            .get(file)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().into_owned())
    }

    fn snapshot_dir(&self, file: usize) -> Option<PathBuf> {
        let p = self.file_paths.get(file)?;
        if p.as_os_str().is_empty() {
            return None;
        }
        Some(p.parent().unwrap_or(Path::new(".")).join("snapshots"))
    }

    /// The `Scope.File` instance for file `idx`, created on first use.
    fn file_scope(&self, idx: usize) -> Rc<RefCell<ScopeState>> {
        self.files
            .borrow_mut()
            .entry(idx)
            .or_insert_with(|| Rc::new(RefCell::new(ScopeState::default())))
            .clone()
    }
}

/// Async so a `ctx:defer` callback can `await` (e.g. `proc:stop()` to kill a spawned process, or any
/// async resource cleanup). Sync callbacks just complete immediately under `call_async`.
///
/// Returns whatever raised, so the caller can report it. **Teardown errors used to be discarded**
/// (`let _ = …`), which mattered far more than "a TODO": `ctx:manage` teardown is what stops
/// containers, so a cleanup that raised was a *leaked container the run reported as green*. The
/// failure could not be seen, only noticed later as a stray container.
///
/// Every teardown still runs even if an earlier one raises. One bad `defer` must not strand the
/// cleanups registered around it, or a single mistake leaks every resource behind it.
#[must_use = "teardown errors must be reported, not dropped — that was the bug this returns to fix"]
async fn teardown_scope(scope: &Rc<RefCell<ScopeState>>) -> Vec<String> {
    let (teardowns, tempdirs) = {
        let mut s = scope.borrow_mut();
        (
            std::mem::take(&mut s.teardowns),
            std::mem::take(&mut s.tempdirs),
        )
    };
    let mut errors = Vec::new();
    // LIFO: last registered runs first, so a fixture's cleanup runs before its dependencies'.
    for f in teardowns.into_iter().rev() {
        if let Err(e) = f.call_async::<()>(()).await {
            errors.push(e.to_string());
        }
    }
    for dir in tempdirs.into_iter().rev() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    errors
}

/// A scope's teardown failures, as their own reported leaf.
///
/// **Why separate leaves rather than blaming a test** (resolving `docs/design/api.md` §Open
/// questions #2). A `Scope.File` fixture tears down after *every* test in the file, so no single
/// test owns the failure — attributing it to one would blame whichever test happened to sort last.
/// And a teardown failure is not the test's fault: it happened *after* the body passed, so turning
/// that test red would report a defect in the wrong place. It is its own event, so it gets its own
/// node: `<scope> ⟶ teardown`, counted in `failed` like any other. That needs no new reporting
/// concept — `Event::NodeFinished` already carries a path, an outcome, and a message.
/// Tear every scope down and report failures to **stderr**, for the paths with no reporter: `eval`,
/// `up`, `watch`, `down`, and partial-provision cleanup.
///
/// They must go somewhere. These are exactly the paths that stop containers, so a swallowed teardown
/// error is a resource still running after prova said it was done — the operator's machine quietly
/// accumulating what a green run promised it had reaped.
async fn teardown_all_and_warn(state: &RunState) {
    let mut late = teardown_file_scopes(state).await;
    late.extend(teardown_results(
        "suite",
        teardown_scope(&state.suite).await,
        None,
        None,
    ));
    for r in &late {
        eprintln!(
            "prova: {} failed: {}",
            r.path,
            r.message.as_deref().unwrap_or("(no message)")
        );
    }
    if !late.is_empty() {
        eprintln!(
            "prova: {} teardown failure(s) — resources may still be running; check `docker ps`",
            late.len()
        );
    }
}

fn teardown_results(
    label: &str,
    errors: Vec<String>,
    file: Option<&str>,
    line: Option<u32>,
) -> Vec<NodeResult> {
    errors
        .into_iter()
        .map(|message| NodeResult {
            path: format!("{label} ⟶ teardown"),
            outcome: Outcome::Failed,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some(message),
            file: file.map(str::to_string),
            line,
            teardown: true,
            spec: None,
        })
        .collect()
}

pub(crate) fn make_tempdir() -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = std::env::temp_dir();
    path.push(format!("prova-{}-{}-{}", std::process::id(), nanos, n));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

// ---------------------------------------------------------------------------------------------
// Collection model
// ---------------------------------------------------------------------------------------------

enum NodeKind {
    Group,
    Flow,
    Test,
}

struct Node {
    name: String,
    kind: NodeKind,
    params: Params,
    opts: UnitOpts,
    children: Vec<NodeIx>,
    body: Option<Function>,
    /// A `test_each` case, delivered to the body as its second argument and as `t.case`. `None` for
    /// ordinary tests (the body simply ignores the extra nil argument).
    case: Option<Value>,
    /// Index of the source file this node was collected from (a suite may load several files into one
    /// state). Set by `Collector::add`; drives per-file `Scope.File`. Always 0 for a single file.
    file: usize,
    /// 1-based line of the declaration call (`prova.test(...)`, `flow:step(...)`) in that file,
    /// captured from the Lua stack at registration. `None` when no frame in the current file was
    /// on the stack (a synthetic node, or a declaration made entirely from a helper chunk).
    line: Option<u32>,
}

struct Collector {
    nodes: Vec<Node>,
    fixtures: Vec<FixtureDef>,
    /// Named topologies (`prova.topology`) → their fixture id, so `prova up <name>` can address a
    /// whole environment by name. A topology is a fixture that is *also* addressable by the `up`/
    /// `start` verbs; in test mode it is used exactly like any other fixture (`t:use(handle)`).
    topologies: BTreeMap<String, usize>,
    /// The stack of ambient parents for *bare* top-level declarations (`prova.test`/`test_each`/
    /// `group`/`flow`). `prova.describe` pushes its labeling group so bare declarations inside its
    /// body nest under it (dynamic scoping); everything pops back to the file root (index 0).
    parent_stack: Vec<NodeIx>,
    /// How many `group`/`flow` builder bodies are currently executing. Non-zero means bare
    /// declarations are a misuse (they would silently register at the file root, outside the unit
    /// being built) — children belong on the builder (`g:test`/`flow:step`), so the bare forms
    /// error instead of registering somewhere the author did not mean.
    builder_depth: usize,
    /// The index of the file currently being loaded (a suite loads several files into one collector).
    /// Every node added while this is set records it, so `Scope.File` can reset per file.
    current_file: usize,
    /// Source path per file index (`file_paths[i]` is the file loaded as index `i`), so a snapshot
    /// assertion can colocate its `.snap` beside the test file it ran from. Grown as files load.
    file_paths: Vec<PathBuf>,
}

impl Collector {
    fn new(root_name: String) -> Self {
        Collector {
            nodes: vec![Node {
                name: root_name,
                kind: NodeKind::Group,
                params: Params::default(),
                opts: UnitOpts::default(),
                children: vec![],
                body: None,
                case: None,
                file: 0,
                line: None,
            }],
            fixtures: vec![],
            topologies: BTreeMap::new(),
            parent_stack: vec![0],
            builder_depth: 0,
            current_file: 0,
            file_paths: Vec::new(),
        }
    }

    /// Record the source path for a file index (idempotent-ish: grows the vec so `file_paths[idx]` is
    /// set). Called as each file loads, before its nodes are collected.
    fn set_file_path(&mut self, idx: usize, path: &Path) {
        if self.file_paths.len() <= idx {
            self.file_paths.resize(idx + 1, PathBuf::new());
        }
        self.file_paths[idx] = path.to_path_buf();
    }

    fn add(&mut self, parent: NodeIx, mut node: Node) -> NodeIx {
        node.file = self.current_file; // stamp every node with the file being loaded
        let ix = self.nodes.len();
        self.nodes.push(node);
        self.nodes[parent].children.push(ix);
        ix
    }

    /// The current ambient parent for a bare top-level declaration.
    fn current_parent(&self) -> NodeIx {
        *self.parent_stack.last().unwrap_or(&0)
    }
}

/// Reject a bare declaration (`prova.test`/`test_each`/`group`/`flow`/`describe`) made while a
/// `group`/`flow` builder body is executing. The bare form would register at the ambient parent —
/// the file root — not inside the unit being built, so the flow would run zero of "its" steps and
/// the tests would lose the parent's ordering/opts. Silently-wrong structure; error instead.
fn reject_bare_in_builder(col: &SharedCollector, what: &str) -> mlua::Result<()> {
    if col.borrow().builder_depth > 0 {
        return Err(mlua::Error::RuntimeError(format!(
            "bare `prova.{what}` inside a group/flow body — declare children on the builder \
             argument instead (`function(g) g:test(...) end` / `function(flow) \
             flow:step(name, fn) end`); the bare form registers at the file root, outside \
             the unit being built"
        )));
    }
    Ok(())
}

type SharedCollector = Rc<RefCell<Collector>>;

fn split_opts_body(a: Value, b: Value) -> mlua::Result<(UnitOpts, Function)> {
    match (a, b) {
        (Value::Function(f), Value::Nil) => Ok((UnitOpts::default(), f)),
        (Value::Table(t), Value::Function(f)) => Ok((parse_opts(&t)?, f)),
        _ => Err(mlua::Error::RuntimeError(
            "expected (name, fn) or (name, opts, fn)".into(),
        )),
    }
}

fn parse_opts(t: &mlua::Table) -> mlua::Result<UnitOpts> {
    let timeout = t
        .get::<Option<String>>("timeout")?
        .and_then(|s| parse_duration(&s));
    let tags = t.get::<Option<Vec<String>>>("tags")?.unwrap_or_default();
    let depends_on = match t.get::<Option<Vec<Value>>>("depends_on")? {
        None => Vec::new(),
        Some(vals) => vals
            .into_iter()
            .map(|v| match v {
                Value::UserData(ud) => ud.borrow::<UnitHandle>().map(|h| h.ix).map_err(|_| {
                    mlua::Error::RuntimeError(
                        "depends_on entries must be unit handles from prova.test/flow/group".into(),
                    )
                }),
                _ => Err(mlua::Error::RuntimeError(
                    "depends_on entries must be unit handles from prova.test/flow/group".into(),
                )),
            })
            .collect::<mlua::Result<Vec<_>>>()?,
    };
    let resources = match t.get::<Option<Vec<Value>>>("resources")? {
        None => Vec::new(),
        Some(vals) => vals
            .into_iter()
            .map(parse_resource)
            .collect::<mlua::Result<Vec<_>>>()?,
    };
    let serial = t.get::<Option<bool>>("serial")?.unwrap_or(false);
    let requires = t
        .get::<Option<Vec<String>>>("requires")?
        .unwrap_or_default();
    let spec = parse_spec_opt(&t.get::<Value>("spec")?)?;
    let proves = parse_proves_opt(&t.get::<Value>("proves")?)?;
    if spec.is_some() && proves.is_some() {
        return Err(mlua::Error::RuntimeError(
            "a test carries spec or proves, not both — while the work is open its context lives in the spec's reason; convert the flag to proves when the spec graduates".into(),
        ));
    }
    Ok(UnitOpts {
        timeout,
        tags,
        depends_on,
        resources,
        serial,
        requires,
        spec,
        proves,
    })
}

/// The `spec` opt: a **non-empty reason string** — the why/ticket behind the still-open
/// contract, forced from day one (a bare `spec = true` tells the burndown nothing, and the
/// reason is what graduates into the `proves` context). There is deliberately no
/// `spec = false` — a test without the flag is already a full proof — so every wrong shape is
/// rejected with the fix, not silently accepted.
fn parse_spec_opt(v: &Value) -> mlua::Result<Option<String>> {
    match v {
        Value::Nil => Ok(None),
        Value::String(s) if !s.to_string_lossy().is_empty() => {
            Ok(Some(s.to_string_lossy().to_string()))
        }
        Value::Boolean(false) => Err(mlua::Error::RuntimeError(
            "spec = false is not a thing — a test without a spec flag is already a full proof; remove the entry".into(),
        )),
        _ => Err(mlua::Error::RuntimeError(
            "spec carries the reason a contract is still open — give it a non-empty string (the why/ticket), or remove the entry".into(),
        )),
    }
}

/// The `proves` opt: graduated context — the why behind a finished proof, living in the test
/// itself. The context IS the point, so a bare `proves = true` or an empty string is refused
/// with the fix rather than accepted as a say-nothing annotation.
fn parse_proves_opt(v: &Value) -> mlua::Result<Option<String>> {
    match v {
        Value::Nil => Ok(None),
        Value::String(s) if !s.to_string_lossy().is_empty() => {
            Ok(Some(s.to_string_lossy().to_string()))
        }
        _ => Err(mlua::Error::RuntimeError(
            "proves carries the context behind a finished proof — give it a non-empty string (the why), or remove the entry".into(),
        )),
    }
}

/// A `resources` entry is a typed `ResourceRef` (a writer or a reader hold) or a bare string (an
/// ad-hoc exclusive token). Anything else is a helpful error rather than a silent no-op.
fn parse_resource(v: Value) -> mlua::Result<ResourceReq> {
    match v {
        Value::String(s) => Ok(ResourceReq {
            token: s.to_string_lossy().to_string(),
            shared: false,
        }),
        Value::UserData(ud) => ud
            .borrow::<ResourceRef>()
            .map(|r| ResourceReq {
                token: r.token.clone(),
                shared: r.shared,
            })
            .map_err(|_| mlua::Error::RuntimeError(RESOURCE_ENTRY_ERR.into())),
        _ => Err(mlua::Error::RuntimeError(RESOURCE_ENTRY_ERR.into())),
    }
}

/// What a `resources` list accepts, said once so the two rejection paths can't drift.
const RESOURCE_ENTRY_ERR: &str =
    "resources entries must be strings or prova.port/writes/reads refs";

/// Build a typed resource ref in `shared` mode from a bare token or an existing ref. Re-moding is
/// deliberate: `prova.reads(prova.port(5432))` widens a port to a concurrent hold.
fn resource_ref(lua: &Lua, v: Value, shared: bool) -> mlua::Result<mlua::AnyUserData> {
    let req = parse_resource(v)?;
    lua.create_userdata(ResourceRef {
        token: req.token,
        shared,
    })
}

// ---------------------------------------------------------------------------------------------
// The context (`t` / `ctx`) — one type for test bodies and fixture factories
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct TestRun {
    assertions: usize,
    failure: Option<String>,
    skip: Option<String>,
    /// Inside `t:expect_all(...)`, a failed assertion is collected here instead of aborting, so the
    /// block reports *every* failure. `soft` is the active flag; `soft_failures` accumulates.
    soft: bool,
    soft_failures: Vec<String>,
    /// Snapshot context for `matches_snapshot` (where `.snap` files live, the key base, update mode,
    /// and a per-test counter for auto-named snapshots). `None` when the test has no source file path.
    snapshot: Option<SnapshotCtx>,
}

/// Per-test snapshot state: everything `matches_snapshot` needs to locate and key a `.snap` file.
struct SnapshotCtx {
    /// `<test-file-dir>/snapshots`.
    dir: PathBuf,
    /// The test-file stem — the `.snap` filename prefix (`<stem>__<key>.snap`).
    stem: String,
    /// A slug of the test's node path — the base for auto-named snapshots (`<slug>-<n>`).
    key_base: String,
    /// `--update-snapshots`: write instead of compare.
    update: bool,
    /// Increments per *unnamed* `matches_snapshot` in this test, so several are distinct.
    counter: usize,
    /// Shared registry to record each referenced `.snap` into (for unreferenced reconciliation).
    registry: Option<SnapshotRegistry>,
}

/// Injected into every body/factory. `own_scope` is the scope its `defer`/`tempdir` target and the
/// floor for the scope-mismatch check; `test_scope` is the active test/step scope instance;
/// `flow_scope` is the enclosing flow's scope instance (present only inside a flow).
///
/// `Clone` is cheap (all fields are `Rc`/`Copy`) and lets the async `use` method own a snapshot in
/// its future without holding the userdata borrow across an `await`.
#[derive(Clone)]
struct Ctx {
    run: Rc<RefCell<TestRun>>,
    state: Rc<RunState>,
    test_scope: Rc<RefCell<ScopeState>>,
    /// This test's file scope instance (`Scope.File`) — its file's, so it is shared across the file's
    /// tests but distinct per file within a suite.
    file_scope: Rc<RefCell<ScopeState>>,
    flow_scope: Option<Rc<RefCell<ScopeState>>>,
    own_scope: ScopeKind,
    /// The `test_each` case for this test, exposed as `t.case` (also passed as the body's 2nd arg).
    /// `None` (→ `nil`) for ordinary tests and for fixture factory contexts.
    case: Option<Value>,
    /// True only for the context injected into a `prova.topology` factory: it makes `ctx.network`
    /// return the topology's ambient managed network (lazily created + scope-managed). Every other
    /// context — test bodies, ordinary `prova.fixture` factories, `prova eval` — leaves it `false`,
    /// so `ctx.network` is nil and resources provisioned there never auto-join a network.
    topology: bool,
}

impl Ctx {
    fn scope_state(&self, kind: ScopeKind) -> mlua::Result<Rc<RefCell<ScopeState>>> {
        Ok(match kind {
            ScopeKind::Suite => self.state.suite.clone(),
            ScopeKind::File => self.file_scope.clone(),
            ScopeKind::Flow => self.flow_scope.clone().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "flow-scoped fixture used outside a flow (flow scope is only valid inside a `prova.flow`)".into(),
                )
            })?,
            ScopeKind::Test => self.test_scope.clone(),
        })
    }
    fn own_scope_state(&self) -> mlua::Result<Rc<RefCell<ScopeState>>> {
        self.scope_state(self.own_scope)
    }
}

/// Resolve `ctx:use(handle|name)` to a fixture value, building it lazily if not cached. Async so a
/// factory can `await` (e.g. `shell.run`, `http.wait_for`). Recursion (a factory that itself calls
/// `ctx:use`) reenters through Lua, not Rust, so no boxing is needed. No `RefCell` borrow is held
/// across the `await`.
async fn resolve_use(lua: &Lua, this: &Ctx, target: Value) -> mlua::Result<Value> {
    let id = match &target {
        Value::UserData(ud) => {
            ud.borrow::<FixtureHandle>()
                .map_err(|_| mlua::Error::RuntimeError("use() expects a fixture handle".into()))?
                .id
        }
        Value::String(s) => {
            let name = s.to_string_lossy();
            this.state
                .defs
                .iter()
                .position(|d| d.name == name)
                .ok_or_else(|| mlua::Error::RuntimeError(format!("no fixture named {name:?}")))?
        }
        _ => {
            return Err(mlua::Error::RuntimeError(
                "use() expects a fixture handle or name".into(),
            ))
        }
    };

    // `get` (not indexing): an eval snippet can mint a handle *after* the run state was built
    // (fixtures registered mid-snippet), so an unknown id must be an error, not a panic.
    let def = this.state.defs.get(id).cloned().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "fixture is not registered in this run (in `prova eval`, a fixture declared inside \
             the snippet cannot be used via ctx:use — call its factory directly)"
                .into(),
        )
    })?;

    // Scope compatibility: a fixture may only use fixtures of equal-or-broader scope.
    if def.scope.rank() < this.own_scope.rank() {
        return Err(mlua::Error::RuntimeError(format!(
            "scope mismatch: {}-scoped fixture {:?} cannot be used by a {}-scoped fixture",
            def.scope.label(),
            def.name,
            this.own_scope.label()
        )));
    }

    let ss = this.scope_state(def.scope)?;
    if let Some(v) = ss.borrow().cache.get(&id) {
        return Ok(v.clone());
    }

    // Build lazily: a child context bound to the fixture's own scope.
    let child = Ctx {
        run: this.run.clone(),
        state: this.state.clone(),
        test_scope: this.test_scope.clone(),
        file_scope: this.file_scope.clone(),
        flow_scope: this.flow_scope.clone(),
        own_scope: def.scope,
        case: None,
        // A topology's factory context is topology-capable: `ctx.network` provisions/serves its
        // ambient managed network. Reached through every terminal verb — `t:use`, `prova up`, and the
        // warm MCP path all provision a topology through this one `resolve_use` seam.
        topology: def.is_topology,
    };
    let child_ud = lua.create_userdata(child)?;
    let value: Value = def.factory.call_async(child_ud).await?;
    ss.borrow_mut().cache.insert(id, value.clone());
    Ok(value)
}

impl UserData for Ctx {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        // `t.case` — the current `test_each` case (nil for ordinary tests).
        fields.add_field_method_get("case", |_, this| {
            Ok(this.case.clone().unwrap_or(Value::Nil))
        });

        // `ctx.network` — the topology's ambient managed docker network. Non-nil ONLY inside a
        // `prova.topology` factory (this is the hard invariant that keeps ordinary fixtures
        // unaffected). Created lazily on first access and cached on the topology's own scope, so
        // repeated reads return the same handle; its teardown is registered right after creation so
        // LIFO reaping removes it after the containers joined to it. Reading it in any non-topology
        // context returns nil, so `prova.containerized`'s `container()` never auto-networks there.
        fields.add_field_method_get("network", |_lua, this| {
            if !this.topology {
                return Ok(Value::Nil);
            }
            let scope = this.own_scope_state()?;
            if let Some(v) = scope.borrow().network.clone() {
                return Ok(v);
            }
            #[cfg(feature = "docker")]
            {
                let net_ud = crate::modules::docker::create_managed_network(_lua)?;
                let net_val = Value::UserData(net_ud);
                // A teardown that removes the network on scope teardown (LIFO → after its containers).
                let teardown: Function = _lua
                    .load("local n = ...\nreturn function() return n:stop() end")
                    .call(net_val.clone())?;
                {
                    let mut s = scope.borrow_mut();
                    s.network = Some(net_val.clone());
                    s.teardowns.push(teardown);
                }
                Ok(net_val)
            }
            #[cfg(not(feature = "docker"))]
            {
                Ok(Value::Nil)
            }
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Async so fixture factories (and test bodies) can `await` while a fixture is built.
        methods.add_async_method("use", |lua, this, target: Value| {
            let ctx = (*this).clone();
            async move { resolve_use(&lua, &ctx, target).await }
        });

        methods.add_method("defer", |_, this, f: Function| {
            this.own_scope_state()?.borrow_mut().teardowns.push(f);
            Ok(())
        });

        // ctx:manage(resource) — tie a resource's lifecycle to this scope: on teardown, call its
        // `stop()` (containers, processes) or `close()` (connections). Returns the resource, so
        // `local pg = ctx:manage(docker.run{...})` both provisions and registers cleanup in one line.
        // Sugar over `ctx:defer`, which remains for anything custom.
        methods.add_method("manage", |lua, this, resource: Value| {
            // Build the teardown closure with the resource captured as an upvalue; it resolves the
            // right method (stop/close) at teardown and awaits it (teardown runs async).
            let teardown: Function = lua
                .load(
                    "local r = ...\n\
                     if (type(r) ~= 'userdata' and type(r) ~= 'table') or not (r.stop or r.close) then\n\
                       error('ctx:manage: resource has no stop() or close() method', 2)\n\
                     end\n\
                     return function()\n\
                       if r.stop then return r:stop() else return r:close() end\n\
                     end",
                )
                .call(resource.clone())?;
            this.own_scope_state()?.borrow_mut().teardowns.push(teardown);
            Ok(resource)
        });

        methods.add_method("tempdir", |_, this, ()| {
            let path = make_tempdir()
                .map_err(|e| mlua::Error::RuntimeError(format!("tempdir failed: {e}")))?;
            let s = path.to_string_lossy().into_owned();
            this.own_scope_state()?.borrow_mut().tempdirs.push(path);
            Ok(s)
        });

        methods.add_method("log", |_, _this, msg: String| {
            // stderr keeps stdout clean for the JSON protocol; will become a Log event later.
            eprintln!("    · {msg}");
            Ok(())
        });

        methods.add_method(
            "expect",
            |lua, this, (subject, label): (Value, Option<String>)| {
                lua.create_userdata(Matcher {
                    subject,
                    label,
                    negated: false,
                    run: this.run.clone(),
                    probe: None,
                })
            },
        );

        methods.add_method("skip", |_, this, reason: String| -> mlua::Result<()> {
            this.run.borrow_mut().skip = Some(reason);
            Err(mlua::Error::RuntimeError(SKIP_SENTINEL.into()))
        });

        // Soft assertions: run `body` collecting every failed assertion instead of aborting on the
        // first, then fail once with all of them. Reports every missing file, not just the first.
        methods.add_method("expect_all", |_, this, body: Function| {
            let prev = {
                let mut r = this.run.borrow_mut();
                std::mem::replace(&mut r.soft, true)
            };
            let outcome = body.call::<()>(());
            let failures = {
                let mut r = this.run.borrow_mut();
                r.soft = prev;
                std::mem::take(&mut r.soft_failures)
            };
            outcome?; // propagate a real error (or a `skip`) raised inside the block
            if failures.is_empty() {
                return Ok(());
            }
            let combined = format!(
                "{} soft assertion(s) failed:\n    - {}",
                failures.len(),
                failures.join("\n    - ")
            );
            this.run.borrow_mut().failure = Some(combined.clone());
            Err(mlua::Error::RuntimeError(combined))
        });
    }
}

const SKIP_SENTINEL: &str = "__prova_skip__";

// ---------------------------------------------------------------------------------------------
// Matchers
// ---------------------------------------------------------------------------------------------

/// One `:eventually` poll observation, deposited by a probe-mode `Matcher`: `(passed, message)`.
type ProbeState = Rc<RefCell<Option<(bool, String)>>>;

struct Matcher {
    subject: Value,
    label: Option<String>,
    negated: bool,
    run: Rc<RefCell<TestRun>>,
    /// `:eventually` probe mode: when set, `record` deposits `(passed, message)` here instead of
    /// counting an assertion or raising — one poll iteration, observed by the retry loop.
    probe: Option<ProbeState>,
}

impl Matcher {
    fn record(&self, raw_pass: bool, detail: impl FnOnce() -> String) -> mlua::Result<()> {
        let passed = raw_pass ^ self.negated;
        if let Some(probe) = &self.probe {
            let msg = if passed {
                String::new()
            } else {
                let prefix = self
                    .label
                    .as_ref()
                    .map(|l| format!("{l}: "))
                    .unwrap_or_default();
                let neg = if self.negated { "not: " } else { "" };
                format!("{prefix}{neg}{}", detail())
            };
            *probe.borrow_mut() = Some((passed, msg));
            return Ok(());
        }
        let mut r = self.run.borrow_mut();
        r.assertions += 1;
        if passed {
            return Ok(());
        }
        let prefix = self
            .label
            .as_ref()
            .map(|l| format!("{l}: "))
            .unwrap_or_default();
        let neg = if self.negated { "not: " } else { "" };
        let msg = format!("{prefix}{neg}{}", detail());
        if r.soft {
            // Inside `expect_all`: collect and keep going.
            r.soft_failures.push(msg);
            Ok(())
        } else {
            r.failure = Some(msg.clone());
            Err(mlua::Error::RuntimeError(msg))
        }
    }
}

/// The `:eventually` handle (docs/plans/api-freeze.md §4): returned by
/// `t:expect(fn):eventually(opts?)`, it dispatches ANY terminal matcher — `__index` hands back an
/// async closure that re-evaluates the function subject and re-runs that matcher (via a
/// probe-mode `Matcher`) until it passes or the deadline lapses. Sugar over the same
/// poll-until-truthy idea as `prova.retry`, which stays the public primitive.
#[derive(Clone)]
struct Eventually {
    func: mlua::Function,
    label: Option<String>,
    negated: bool,
    run: Rc<RefCell<TestRun>>,
    timeout: Duration,
    every: Duration,
}

impl UserData for Eventually {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::Index, |lua, this, name: String| {
            let ev = this.clone();
            lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let ev = ev.clone();
                let name = name.clone();
                async move {
                    // `ev:gte(3)` sugars to `f(ev, 3)`: drop the handle, keep the matcher args.
                    let rest: mlua::MultiValue = args.into_iter().skip(1).collect();
                    // Lua-semantics dispatch onto a probe matcher: `m[name](m, ...)`.
                    let dispatch: mlua::Function = lua
                        .load("return function(m, name, ...) return m[name](m, ...) end")
                        .eval()?;
                    let deadline = Instant::now() + ev.timeout;
                    let mut last = format!("the probe was never evaluated (timeout {:?})", ev.timeout);
                    loop {
                        // Re-evaluate the subject; a raise means "not yet", exactly like prova.retry.
                        match ev.func.call_async::<Value>(()).await {
                            Ok(value) => {
                                let state = Rc::new(RefCell::new(None));
                                let probe = lua.create_userdata(Matcher {
                                    subject: value,
                                    label: ev.label.clone(),
                                    negated: ev.negated,
                                    run: ev.run.clone(),
                                    probe: Some(state.clone()),
                                })?;
                                // A raise from the matcher itself (bad arguments) is a programming
                                // error — propagate, never retry.
                                let mut call_args = mlua::MultiValue::new();
                                call_args.push_back(Value::UserData(probe));
                                call_args.push_back(Value::String(lua.create_string(&name)?));
                                for v in rest.clone() {
                                    call_args.push_back(v);
                                }
                                dispatch.call_async::<()>(call_args).await?;
                                let observed = state.borrow_mut().take();
                                match observed {
                                    Some((true, _)) => {
                                        // Honored: one real assertion for the whole poll.
                                        let real = Matcher {
                                            subject: Value::Nil,
                                            label: None,
                                            negated: false,
                                            run: ev.run.clone(),
                                            probe: None,
                                        };
                                        return real.record(true, String::new);
                                    }
                                    Some((false, msg)) => last = msg,
                                    // The dispatched method never recorded (e.g. `never`, which
                                    // returns a new matcher): not a terminal matcher — refuse.
                                    None => {
                                        return Err(mlua::Error::RuntimeError(format!(
                                            "eventually:{name} is not a terminal matcher — apply modifiers before :eventually()",
                                        )));
                                    }
                                }
                            }
                            Err(err) => last = err.to_string(),
                        }
                        if Instant::now() >= deadline {
                            let real = Matcher {
                                subject: Value::Nil,
                                label: None,
                                negated: false,
                                run: ev.run.clone(),
                                probe: None,
                            };
                            let timeout = ev.timeout;
                            return real.record(false, move || {
                                format!("eventually timed out after {timeout:?} — last: {last}")
                            });
                        }
                        tokio::time::sleep(ev.every).await;
                    }
                }
            })
        });
    }
}

/// Serialize a `matches_snapshot` subject to the string that gets stored/compared, honoring the
/// **level** dial. A string subject is its own content. A **filesystem subject** — any Lua table with
/// a `path` string field (the convention every prova path-handle follows: `archetect.render` output,
/// `out:file(...)`, `out:dir(...)`) — serializes at a level:
///
/// - `layout` — the sorted relative file paths (the render's *shape*; stable, low-rot). Default for a
///   directory subject.
/// - `content` — the paths plus each file's bytes, as `=== path ===` sections. Default for a *file*
///   subject (a single file has one content and no meaningful "layout").
///
/// The default-by-kind is the anti-rot guard: a broad directory snapshot defaults to the cheap shape,
/// and you *opt into* `content`.
fn serialize_snapshot_subject(subject: &Value, level: Option<&str>) -> Result<String, String> {
    match subject {
        Value::String(s) => Ok(s.to_string_lossy().to_string()),
        Value::Table(t) => {
            let path: Option<String> = t.get("path").ok().flatten();
            let path = path.ok_or_else(|| {
                "matches_snapshot: table subject must be a path handle (a `path` field); \
                 got a table without one"
                    .to_string()
            })?;
            serialize_path(Path::new(&path), level)
        }
        other => Err(format!(
            "matches_snapshot expects a string or a filesystem path-handle subject, got {}",
            other.type_name()
        )),
    }
}

/// Serialize a filesystem path at a snapshot level (see [`serialize_snapshot_subject`]).
fn serialize_path(path: &Path, level: Option<&str>) -> Result<String, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("matches_snapshot: cannot stat {}: {e}", path.display()))?;

    if meta.is_file() {
        // A single file: `content` is the only meaningful level.
        if matches!(level, Some("layout")) {
            return Err(format!(
                "matches_snapshot: level=\"layout\" needs a directory subject, but {} is a file",
                path.display()
            ));
        }
        return std::fs::read_to_string(path)
            .map_err(|e| format!("matches_snapshot: cannot read {}: {e}", path.display()));
    }

    // A directory: default to the low-rot `layout` (shape), opt into `content`.
    let rels = walk_files_relative(path)?;
    match level.unwrap_or("layout") {
        "layout" => Ok(rels.join("\n")),
        "content" => {
            let mut out = String::new();
            for rel in &rels {
                let full = path.join(rel);
                let body = std::fs::read_to_string(&full)
                    .unwrap_or_else(|_| "<binary or unreadable>".to_string());
                out.push_str(&format!("=== {rel} ===\n{body}\n"));
            }
            Ok(out.trim_end().to_string())
        }
        other => Err(format!(
            "matches_snapshot: unknown level {other:?} (expected \"layout\" or \"content\")"
        )),
    }
}

/// Every file under `root`, as `/`-separated relative paths, sorted — a deterministic layout listing.
fn walk_files_relative(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("matches_snapshot: cannot read dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("matches_snapshot: dir entry error: {e}"))?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A filesystem-safe slug of a node path (or a user-given snapshot name): alphanumerics kept,
/// everything else collapsed to single `-`, lowercased. `"orders › creates a row"` → `"orders-creates-a-row"`.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            out.extend(c.to_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "snapshot".to_string()
    } else {
        out
    }
}

/// The stored `.snap` document: a small header (for review context) then a `---` line, then the raw
/// body. The lone `---` delimiter is robust — a body starting with `#!/bin/sh` or containing later
/// `---` lines round-trips, since only the *first* `---` splits header from body.
fn format_snapshot(source: &str, body: &str) -> String {
    format!("prova-snapshot v1\nsource: {source}\n---\n{body}")
}

/// Extract the body from a stored `.snap` document (everything after the first lone `---` line). A
/// document with no delimiter (hand-written / legacy) is treated as all-body.
fn snapshot_body(doc: &str) -> &str {
    match doc.split_once("\n---\n") {
        Some((_header, body)) => body,
        None => doc,
    }
}

/// Write a snapshot document, creating the `snapshots/` dir if needed. Returns a message on failure.
fn write_snapshot(path: &Path, doc: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create snapshot dir {}: {e}", dir.display()))?;
    }
    std::fs::write(path, doc).map_err(|e| format!("cannot write snapshot {}: {e}", path.display()))
}

/// A minimal LCS-based line diff (`  ` context, `- ` expected-only, `+ ` actual-only), for the
/// snapshot mismatch message. O(n·m) — fine for snapshot-sized inputs.
fn line_diff(expected: &str, actual: &str) -> String {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = actual.lines().collect();
    let (n, m) = (a.len(), b.len());
    // dp[i][j] = LCS length of a[i..] and b[j..].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push_str(&format!("    {}\n", a[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push_str(&format!("  - {}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("  + {}\n", b[j]));
            j += 1;
        }
    }
    for line in &a[i..] {
        out.push_str(&format!("  - {line}\n"));
    }
    for line in &b[j..] {
        out.push_str(&format!("  + {line}\n"));
    }
    out.trim_end().to_string()
}

impl UserData for Matcher {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("never", |lua, this, ()| {
            lua.create_userdata(Matcher {
                subject: this.subject.clone(),
                label: this.label.clone(),
                negated: !this.negated,
                run: this.run.clone(),
                probe: this.probe.clone(),
            })
        });

        // `:eventually(opts?)` — poll-until-matches (docs/plans/api-freeze.md §4). Legal only on
        // a FUNCTION subject: the returned handle re-evaluates it (and the terminal matcher that
        // follows) until pass or timeout. `opts = { timeout, every }`, defaults matching
        // `prova.retry` — which remains the public primitive this sugars over.
        methods.add_method("eventually", |lua, this, opts: Option<Table>| {
            let Value::Function(func) = &this.subject else {
                return Err(mlua::Error::RuntimeError(
                    "eventually requires a function subject — wrap the probe: t:expect(function() return ... end):eventually():matches{...}"
                        .into(),
                ));
            };
            let get = |key: &str, default: Duration| -> mlua::Result<Duration> {
                match &opts {
                    Some(t) => match t.get::<Option<String>>(key)? {
                        Some(s) => parse_duration(&s).ok_or_else(|| {
                            mlua::Error::RuntimeError(format!(
                                "eventually: cannot parse {key} {s:?} (try \"30s\", \"500ms\")"
                            ))
                        }),
                        None => Ok(default),
                    },
                    None => Ok(default),
                }
            };
            let timeout = get("timeout", Duration::from_secs(30))?;
            let every = get("every", Duration::from_millis(500))?;
            lua.create_userdata(Eventually {
                func: func.clone(),
                label: this.label.clone(),
                negated: this.negated,
                run: this.run.clone(),
                timeout,
                every,
            })
        });

        methods.add_method("equals", |_, this, other: Value| {
            let pass = values_equal(&this.subject, &other);
            this.record(pass, || {
                format!(
                    "expected {}, got {}",
                    display(&other),
                    display(&this.subject)
                )
            })
        });
        methods.add_method("eq", |_, this, other: Value| {
            let pass = values_equal(&this.subject, &other);
            this.record(pass, || {
                format!(
                    "expected {}, got {}",
                    display(&other),
                    display(&this.subject)
                )
            })
        });

        // Compare the subject against a stored `.snap` file colocated with the test
        // (`<dir>/snapshots/<file-stem>__<key>.snap`). `--update-snapshots` (re)writes it and passes;
        // otherwise a mismatch fails with a line diff and a missing snapshot fails after writing a
        // reviewable `.snap.new`. `arg` is nil, a name string, or an options table `{ name, level }`
        // (Phase A takes only a string subject + name; `level`/tree subjects come with the tree phase).
        methods.add_method("matches_snapshot", |_, this, arg: Value| {
            if this.negated {
                return Err(mlua::Error::RuntimeError(
                    "matches_snapshot cannot be negated".into(),
                ));
            }
            // `arg` is nil | a name string | an options table `{ name?, level? }`.
            let (name, level): (Option<String>, Option<String>) = match arg {
                Value::Nil => (None, None),
                Value::String(s) => (Some(s.to_string_lossy().to_string()), None),
                Value::Table(t) => (t.get::<Option<String>>("name")?, t.get::<Option<String>>("level")?),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "matches_snapshot(name?) expects a string name or an options table, got {}",
                        other.type_name()
                    )))
                }
            };
            let actual = serialize_snapshot_subject(&this.subject, level.as_deref())
                .map_err(mlua::Error::RuntimeError)?;

            // Resolve the `.snap`/`.snap.new` paths + update flag + a header source label from the
            // per-test snapshot context (advancing the auto-name counter for an unnamed snapshot).
            let (snap, snap_new, update, source, registry) = {
                let mut r = this.run.borrow_mut();
                let ctx = r.snapshot.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "matches_snapshot needs a test-file context (no source path recorded for this run)"
                            .into(),
                    )
                })?;
                let key = match &name {
                    Some(n) => slugify(n),
                    None => {
                        ctx.counter += 1;
                        format!("{}-{}", ctx.key_base, ctx.counter)
                    }
                };
                let base = format!("{}__{}", ctx.stem, key);
                (
                    ctx.dir.join(format!("{base}.snap")),
                    ctx.dir.join(format!("{base}.snap.new")),
                    ctx.update,
                    format!("{} / {}", ctx.key_base, key),
                    ctx.registry.clone(),
                )
            };

            // Record this `.snap` as referenced (whatever the outcome), so an unreferenced-snapshot
            // reconcile can tell orphaned files from ones a test still points at.
            if let Some(reg) = &registry {
                if let Ok(mut set) = reg.lock() {
                    set.insert(snap.clone());
                }
            }

            let stored_doc = format_snapshot(&source, &actual);

            if update {
                if let Err(e) = write_snapshot(&snap, &stored_doc) {
                    return Err(mlua::Error::RuntimeError(e));
                }
                let _ = std::fs::remove_file(&snap_new); // accepted → drop any pending .new
                return this.record(true, String::new);
            }

            match std::fs::read_to_string(&snap) {
                Ok(doc) => {
                    let expected = snapshot_body(&doc);
                    if expected == actual {
                        let _ = std::fs::remove_file(&snap_new);
                        this.record(true, String::new)
                    } else {
                        let _ = write_snapshot(&snap_new, &stored_doc);
                        let diff = line_diff(expected, &actual);
                        let path = snap.display().to_string();
                        this.record(false, move || {
                            format!(
                                "snapshot mismatch ({path})\n{diff}\n  \
                                 run `prova --update-snapshots` to accept, or see the .snap.new"
                            )
                        })
                    }
                }
                Err(_) => {
                    let _ = write_snapshot(&snap_new, &stored_doc);
                    let path = snap.display().to_string();
                    this.record(false, move || {
                        format!(
                            "no snapshot at {path} — wrote {path}.new; \
                             run `prova --update-snapshots` to accept it"
                        )
                    })
                }
            }
        });
        // Identity, not structure: the *same* table/function/userdata (reference), or an equal
        // primitive (`rawequal` semantics). Complements the **deep** `equals` — use `is` to assert
        // "this is that same object", including tables that hold function fields `equals` can't compare.
        methods.add_method("is", |_, this, other: Value| {
            let pass = this.subject == other;
            this.record(pass, || {
                format!(
                    "expected {} to be (identity) {}",
                    display(&this.subject),
                    display(&other)
                )
            })
        });
        methods.add_method("is_true", |_, this, ()| {
            let pass = matches!(this.subject, Value::Boolean(true));
            this.record(pass, || {
                format!("expected true, got {}", display(&this.subject))
            })
        });
        methods.add_method("is_false", |_, this, ()| {
            let pass = matches!(this.subject, Value::Boolean(false));
            this.record(pass, || {
                format!("expected false, got {}", display(&this.subject))
            })
        });
        methods.add_method("is_nil", |_, this, ()| {
            let pass = matches!(this.subject, Value::Nil);
            this.record(pass, || {
                format!("expected nil, got {}", display(&this.subject))
            })
        });
        methods.add_method("is_truthy", |_, this, ()| {
            let pass = truthy(&this.subject);
            this.record(pass, || {
                format!("expected a truthy value, got {}", display(&this.subject))
            })
        });
        methods.add_method("contains", |_, this, needle: Value| {
            let pass = contains(&this.subject, &needle);
            this.record(pass, || {
                format!(
                    "expected {} to contain {}",
                    display(&this.subject),
                    display(&needle)
                )
            })
        });

        // Filesystem matchers: the subject is a path string (e.g. `t:expect(dir.."/Cargo.toml")`).
        methods.add_method("exists", |_, this, ()| {
            let pass = subject_path(&this.subject).is_some_and(|p| p.exists());
            this.record(pass, || {
                format!("expected path {} to exist", display(&this.subject))
            })
        });
        methods.add_method("is_file", |_, this, ()| {
            let pass = subject_path(&this.subject).is_some_and(|p| p.is_file());
            this.record(pass, || {
                format!("expected {} to be a file", display(&this.subject))
            })
        });
        methods.add_method("is_dir", |_, this, ()| {
            let pass = subject_path(&this.subject).is_some_and(|p| p.is_dir());
            this.record(pass, || {
                format!("expected {} to be a directory", display(&this.subject))
            })
        });
        // Empty means empty for whatever the subject IS: a string with no bytes, a table with no
        // entries, or a path with no children. It read as filesystem-only, so `expect(""):is_empty()`
        // and `expect({}):is_empty()` both FAILED — reporting `expected "" to be empty` about an
        // empty string, which no one can act on. `has_length(0)` already worked on both, so the
        // inconsistency was the bug, not the expectation.
        methods.add_method("is_empty", |_, this, ()| {
            // A string is ambiguous — it may be a path OR a literal. Resolve it by what is on
            // disk: an existing path is a filesystem check (the long-standing behaviour), anything
            // else is its byte length. So `expect(dir):is_empty()` still asks the filesystem, and
            // `expect(""):is_empty()` finally answers about the string.
            let pass = match &this.subject {
                Value::Table(t) => t.clone().pairs::<Value, Value>().next().is_none(),
                Value::String(s) if subject_path(&this.subject).is_none_or(|p| !p.exists()) => {
                    s.as_bytes().is_empty()
                }
                other => path_is_empty(other),
            };
            this.record(pass, || {
                format!("expected {} to be empty", display(&this.subject))
            })
        });

        // The signature archetype check: every file under a rendered tree (a path string, or a
        // tree/dir handle with a `path`) must be free of leftover template markers — no `{{`, `{%`,
        // or `{#` in file *contents* or *path segments*. GitHub Actions `${{ … }}` expressions are
        // legitimately present in rendered workflows, so they are excluded. Tedious to hand-roll
        // (glob every file, read, scan); one call here.
        methods.add_method("is_fully_rendered", |_, this, ()| {
            let offenders = match subject_path(&this.subject) {
                Some(p) => unrendered_markers(&p),
                None => vec!["subject is not a path or tree handle".to_string()],
            };
            let pass = offenders.is_empty();
            this.record(pass, || {
                let shown = offenders
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n    ");
                let more = if offenders.len() > 10 {
                    format!("\n    … and {} more", offenders.len() - 10)
                } else {
                    String::new()
                };
                format!(
                    "expected {} to be fully rendered, but found unrendered template markers:\n    {shown}{more}",
                    display(&this.subject)
                )
            })
        });

        methods.add_method("is_falsy", |_, this, ()| {
            let pass = !truthy(&this.subject);
            this.record(pass, || {
                format!("expected a falsy value, got {}", display(&this.subject))
            })
        });

        // Polymorphic on the argument (the `contains` precedent — docs/plans/api-freeze.md §3):
        // a STRING is a Lua pattern match on a string subject (delegates to `string.find`); a
        // TABLE is a recursive structural SUBSET — every key in the shape must exist in the
        // subject and recursively match, extra subject keys ignored, arrays same-index. One
        // semantics for every surface that matches shapes; spec: proofs/spec/matching/.
        methods.add_method("matches", |lua, this, arg: Value| match arg {
            Value::String(pattern) => {
                let pattern = pattern.to_str()?.to_string();
                let (pass, subject) = match &this.subject {
                    Value::String(s) => {
                        let subject = s.to_str()?.to_string();
                        let find: mlua::Function =
                            lua.globals().get::<Table>("string")?.get("find")?;
                        let found: Value = find.call((subject.clone(), pattern.clone()))?;
                        (!matches!(found, Value::Nil), subject)
                    }
                    other => (false, display(other)),
                };
                this.record(pass, || {
                    format!("expected {subject:?} to match pattern {pattern:?}")
                })
            }
            Value::Table(shape) => {
                let mismatch = match &this.subject {
                    Value::Table(subject) => subset_mismatch(&shape, subject, &mut Vec::new()),
                    other => Some(format!("expected a table, got {}", display(other))),
                };
                let pass = mismatch.is_none();
                this.record(pass, || match mismatch {
                    Some(detail) => format!("does not match shape — {detail}"),
                    None => "matches the shape".to_string(),
                })
            }
            _ => Err(mlua::Error::RuntimeError(
                "matches takes a Lua pattern (string) or a shape (table)".into(),
            )),
        });

        methods.add_method("has_length", |_, this, n: i64| {
            let len = value_length(&this.subject);
            this.record(len == Some(n), || match len {
                Some(l) => format!("expected length {n}, got {l}"),
                None => format!(
                    "expected a string/table of length {n}, got {}",
                    display(&this.subject)
                ),
            })
        });

        methods.add_method("is_one_of", |_, this, options: Table| {
            let mut pass = false;
            for item in options.sequence_values::<Value>() {
                if values_equal(&this.subject, &item?) {
                    pass = true;
                    break;
                }
            }
            this.record(pass, || {
                format!(
                    "expected {} to be one of the given options",
                    display(&this.subject)
                )
            })
        });

        methods.add_method("gt", |_, this, n: f64| {
            let pass = as_number(&this.subject).is_some_and(|x| x > n);
            this.record(pass, || {
                format!("expected {} > {n}", display(&this.subject))
            })
        });
        methods.add_method("gte", |_, this, n: f64| {
            let pass = as_number(&this.subject).is_some_and(|x| x >= n);
            this.record(pass, || {
                format!("expected {} >= {n}", display(&this.subject))
            })
        });
        methods.add_method("lt", |_, this, n: f64| {
            let pass = as_number(&this.subject).is_some_and(|x| x < n);
            this.record(pass, || {
                format!("expected {} < {n}", display(&this.subject))
            })
        });
        methods.add_method("lte", |_, this, n: f64| {
            let pass = as_number(&this.subject).is_some_and(|x| x <= n);
            this.record(pass, || {
                format!("expected {} <= {n}", display(&this.subject))
            })
        });
    }
}

/// A `Value` interpreted as a filesystem path: a string, or a handle table with a `path` field
/// (as returned by `archetect.render(...)` — `t:expect(out:file("Cargo.toml")):exists()`).
fn subject_path(v: &Value) -> Option<PathBuf> {
    match v {
        Value::String(s) => s.to_str().ok().map(|bs| PathBuf::from(&*bs)),
        Value::Table(t) => t
            .get::<Option<String>>("path")
            .ok()
            .flatten()
            .map(PathBuf::from),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------------------------------

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Boolean(false))
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Boolean(x), Value::Boolean(y)) => x == y,
        (Value::Integer(x), Value::Integer(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Integer(x), Value::Number(y)) | (Value::Number(y), Value::Integer(x)) => {
            (*x as f64) == *y
        }
        (Value::String(x), Value::String(y)) => x.to_string_lossy() == y.to_string_lossy(),
        (Value::Table(x), Value::Table(y)) => tables_equal(x, y),
        // Sentinels (json.null) and other lightuserdata compare by identity — what makes
        // `t:expect({ x = json.null }):matches{ x = json.null }` hold (api-freeze §3).
        (Value::LightUserData(x), Value::LightUserData(y)) => x == y,
        _ => false,
    }
}

/// The structural-subset walk behind `:matches(shape)`: every key present in `shape` must exist
/// in `subject` and recursively match; extra subject keys are ignored; an array is just integer
/// keys, so elements match same-index (a shape array shorter than the subject's passes, longer
/// fails on the missing index). Scalar leaves compare with `values_equal` (int↔float coercion).
/// Returns the FIRST mismatch as a `path: expected X, got Y` line — the table-aware diff that
/// pinpoints `status.readyReplicas: expected 3, got 1` instead of `<table> != <table>`.
fn subset_mismatch(shape: &Table, subject: &Table, path: &mut Vec<String>) -> Option<String> {
    for pair in shape.clone().pairs::<Value, Value>() {
        let Ok((key, expected)) = pair else {
            return Some(format!("{}: unreadable shape entry", path_str(path)));
        };
        path.push(key_segment(&key));
        let actual: Value = subject.get::<Value>(key).unwrap_or(Value::Nil);
        let mismatch = match (&expected, &actual) {
            (Value::Table(es), Value::Table(actual_t)) => subset_mismatch(es, actual_t, path),
            _ if values_equal(&expected, &actual) => None,
            (_, Value::Nil) => Some(format!(
                "{}: expected {}, got nothing",
                path_str(path),
                display(&expected)
            )),
            _ => Some(format!(
                "{}: expected {}, got {}",
                path_str(path),
                display(&expected),
                display(&actual)
            )),
        };
        path.pop();
        if mismatch.is_some() {
            return mismatch;
        }
    }
    None
}

/// One path segment for the subset diff: array indices render as `[i]`, string keys as-is.
fn key_segment(key: &Value) -> String {
    match key {
        Value::Integer(i) => format!("[{i}]"),
        Value::String(s) => s.to_string_lossy().to_string(),
        other => format!("[{}]", display(other)),
    }
}

/// Join diff path segments: dots between named keys, indices appended (`status.conditions[1].type`).
fn path_str(path: &[String]) -> String {
    if path.is_empty() {
        return "(root)".to_string();
    }
    let mut out = String::new();
    for seg in path {
        if !seg.starts_with('[') && !out.is_empty() {
            out.push('.');
        }
        out.push_str(seg);
    }
    out
}

/// Deep table equality: same set of keys, values recursively equal. (Cyclic tables are not guarded
/// — test data is expected to be acyclic.)
fn tables_equal(x: &Table, y: &Table) -> bool {
    let mut x_keys = 0;
    for pair in x.clone().pairs::<Value, Value>() {
        let Ok((key, xv)) = pair else { return false };
        x_keys += 1;
        match y.get::<Value>(key) {
            Ok(yv) if values_equal(&xv, &yv) => {}
            _ => return false,
        }
    }
    // Equal key counts (with every x-key matched in y) means no extra keys on either side.
    let y_keys = y.clone().pairs::<Value, Value>().count();
    x_keys == y_keys
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

/// Length of a string (bytes, matching Lua `#`) or a table (sequence length).
fn value_length(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => Some(s.as_bytes().len() as i64),
        Value::Table(t) => Some(t.raw_len() as i64),
        _ => None,
    }
}

/// `is_empty` on a path subject: an empty directory, or a zero-byte file. A non-path (or missing
/// path) is not empty.
fn path_is_empty(v: &Value) -> bool {
    let Some(path) = subject_path(v) else {
        return false;
    };
    if path.is_dir() {
        std::fs::read_dir(&path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    } else {
        std::fs::metadata(&path)
            .map(|m| m.len() == 0)
            .unwrap_or(false)
    }
}

/// Byte index of the first unrendered jinja marker (`{{`, `{%`, `{#`) in `s` that is *not* part of a
/// GitHub Actions `${{ … }}` expression (i.e. not immediately preceded by `$`). `None` if clean.
fn first_marker(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && matches!(b[i + 1], b'{' | b'%' | b'#') {
            let preceded_by_dollar = i > 0 && b[i - 1] == b'$';
            if !preceded_by_dollar {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Every leftover-template-marker offender under `root` — an unrendered `{{`/`{%`/`{#` in a file's
/// contents (reported as `relpath:line: snippet`) or in a path segment (`relpath (unrendered path
/// segment)`). Binary/unreadable files are skipped. A missing `root` is itself an offender.
fn unrendered_markers(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if !root.exists() {
        return vec![format!("{}: path does not exist", root.display())];
    }
    let scan_file = |path: &Path, rel: &Path, out: &mut Vec<String>| {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Some(idx) = first_marker(&contents) {
                let line = contents[..idx].matches('\n').count() + 1;
                let snippet: String = contents[idx..]
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(60)
                    .collect();
                out.push(format!("{}:{line}: {snippet}", rel.display()));
            }
        }
    };
    if root.is_file() {
        scan_file(
            root,
            Path::new(root.file_name().unwrap_or_default()),
            &mut out,
        );
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            // An unrendered *name* (only the segment's own name, so a bad parent isn't re-reported
            // for every child).
            if first_marker(&entry.file_name().to_string_lossy()).is_some() {
                out.push(format!("{} (unrendered path segment)", rel.display()));
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                scan_file(&path, &rel, &mut out);
            }
        }
    }
    out.sort();
    out
}

fn contains(subject: &Value, needle: &Value) -> bool {
    match subject {
        Value::String(s) => match needle {
            Value::String(n) => s.to_string_lossy().contains(&*n.to_string_lossy()),
            _ => false,
        },
        Value::Table(t) => {
            for (_, v) in t.clone().pairs::<Value, Value>().flatten() {
                if values_equal(&v, needle) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn display(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("{:?}", s.to_string_lossy()),
        Value::Table(_) => "<table>".into(),
        Value::Function(_) => "<function>".into(),
        other => format!("<{}>", other.type_name()),
    }
}

// ---------------------------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------------------------

fn build_lua(root_name: String, config: &RunConfig) -> mlua::Result<(Lua, SharedCollector)> {
    let col: SharedCollector = Rc::new(RefCell::new(Collector::new(root_name)));
    let lua = Lua::new();

    // Route `os.getenv` through Rust's view of the environment.
    //
    // Lua's own `os.getenv` reads the C runtime's copy of the environment. On Windows that copy is
    // a snapshot taken at startup, and `std::env::set_var` (SetEnvironmentVariableW) does not
    // update it — so a manifest's `[run.env]`, which we inject with set_var, reached spawned child
    // processes but was invisible to the tests themselves. On Unix the two views are the same table,
    // which is why only Windows saw it. Reading through Rust makes `os.getenv` agree everywhere, and
    // agree with what `shell.run` children inherit.
    {
        let os: mlua::Table = lua.globals().get("os")?;
        os.set(
            "getenv",
            lua.create_function(|_, name: String| Ok(std::env::var(name).ok()))?,
        )?;
    }

    let prova = lua.create_table()?;

    {
        let col = col.clone();
        prova.set(
            "test",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                reject_bare_in_builder(&col, "test")?;
                let parent = col.borrow().current_parent();
                let line = caller_line(lua, &col);
                let ix = register_test(&col, parent, name, a, b, None, line)?;
                lua.create_userdata(UnitHandle { ix })
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "test_each",
            lua.create_function(
                move |lua, (name, cases, factory): (String, Table, Function)| {
                    reject_bare_in_builder(&col, "test_each")?;
                    let parent = col.borrow().current_parent();
                    register_test_each(lua, &col, parent, name, cases, factory)
                },
            )?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "group",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                reject_bare_in_builder(&col, "group")?;
                let parent = col.borrow().current_parent();
                let ix = register_group(lua, &col, parent, name, a, b)?;
                lua.create_userdata(UnitHandle { ix })
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "flow",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                reject_bare_in_builder(&col, "flow")?;
                let parent = col.borrow().current_parent();
                let ix = register_flow(lua, &col, parent, name, a, b)?;
                lua.create_userdata(UnitHandle { ix })
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "describe",
            lua.create_function(move |lua, (label, body): (String, Function)| {
                reject_bare_in_builder(&col, "describe")?;
                register_describe(lua, &col, label, body)
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "fixture",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                let (scope, factory) = match (a, b) {
                    (Value::Function(f), Value::Nil) => (ScopeKind::Test, f),
                    (scope_val, Value::Function(f)) => (parse_scope(scope_val)?, f),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "fixture(name, scope, factory)".into(),
                        ))
                    }
                };
                let id = {
                    let mut c = col.borrow_mut();
                    let id = c.fixtures.len();
                    c.fixtures.push(FixtureDef {
                        name,
                        scope,
                        factory,
                        is_topology: false,
                    });
                    id
                };
                lua.create_userdata(FixtureHandle { id })
            })?,
        )?;
    }
    {
        // prova.topology(name, [scope,] factory) — a named, verb-agnostic bundle of wired resources.
        // It is a fixture (default `Scope.File`, so it is provisioned once and shared across a file's
        // tests) that is *also* addressable by name: `prova up <name>` / `prova start <name>` stand up
        // the identical object outside any test. In test mode it is used like any fixture:
        // `t:use(env)`. Same definition, different terminal verb — tests and dev-env cannot drift.
        let col = col.clone();
        prova.set(
            "topology",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                let (scope, factory) = match (a, b) {
                    (Value::Function(f), Value::Nil) => (ScopeKind::File, f),
                    (scope_val, Value::Function(f)) => (parse_scope(scope_val)?, f),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "topology(name, [scope,] factory)".into(),
                        ))
                    }
                };
                let id = {
                    let mut c = col.borrow_mut();
                    if c.topologies.contains_key(&name) {
                        return Err(mlua::Error::RuntimeError(format!(
                            "topology {name:?} is already defined"
                        )));
                    }
                    let id = c.fixtures.len();
                    c.fixtures.push(FixtureDef {
                        name: name.clone(),
                        scope,
                        factory,
                        is_topology: true,
                    });
                    c.topologies.insert(name, id);
                    id
                };
                lua.create_userdata(FixtureHandle { id })
            })?,
        )?;
    }

    prova.set(
        "sleep",
        lua.create_async_function(|_, millis: u64| async move {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            Ok(())
        })?,
    )?;

    // prova.retry(fn, { timeout = "30s", every = "500ms", message? }) — call `fn` until it returns a
    // truthy value (raising is treated as "not yet"), or the deadline elapses. Returns the value.
    // Replaces the hand-rolled `for _=1,N do pcall(...) sleep end` readiness loop; the common case is
    // waiting for a freshly-provisioned dependency to accept connections.
    prova.set(
        "retry",
        lua.create_async_function(|_, (f, opts): (Function, Option<Table>)| async move {
            let mut timeout = Duration::from_secs(30);
            let mut every = Duration::from_millis(500);
            let mut message: Option<String> = None;
            if let Some(opts) = &opts {
                if let Some(t) = opts
                    .get::<Option<String>>("timeout")?
                    .and_then(|s| parse_duration(&s))
                {
                    timeout = t;
                }
                if let Some(e) = opts
                    .get::<Option<String>>("every")?
                    .and_then(|s| parse_duration(&s))
                {
                    every = e;
                }
                message = opts.get::<Option<String>>("message")?;
            }
            let deadline = Instant::now() + timeout;
            // No initializer: every arm of the match below that reaches the deadline check
            // assigns it, and rustc's definite-assignment analysis proves that.
            let mut last_err: Option<String>;
            loop {
                match f.call_async::<Value>(()).await {
                    Ok(v) if truthy(&v) => return Ok(v),
                    // A falsy return is "not ready" — and it CLEARS any earlier error. Without this,
                    // `last_err` is sticky: a closure that raised at second 1 and merely returned nil
                    // thereafter reports that stale error at the deadline, describing a state that
                    // stopped being true long ago. (Measured in the wild: it sent a caller off
                    // "fixing" a system that was already correct.)
                    Ok(_) => last_err = None,
                    Err(e) => last_err = Some(e.to_string()),
                }
                if Instant::now() >= deadline {
                    let base = message.unwrap_or_else(|| {
                        format!("prova.retry: condition not met within {timeout:?}")
                    });
                    return Err(mlua::Error::RuntimeError(match last_err {
                        Some(e) => format!("{base} (last error: {e})"),
                        // Nothing raised, so the closure simply never returned anything truthy. Say
                        // which of the two it was: "condition not met" alone reads as "the system
                        // never got there", when the actual cause is often a closure that asserts and
                        // forgets to return.
                        None => format!(
                            "{base} (the closure never returned a truthy value — `retry` waits for a \
                             TRUTHY RETURN, so a closure that only asserts must end with `return true`)"
                        ),
                    }));
                }
                tokio::time::sleep(every).await;
            }
        })?,
    )?;

    // Typed resource constructors, named by the ACCESS MODE they take on a token: `writes` is an
    // exclusive (writer) hold, `reads` a concurrent (reader) one. Both accept a bare token *or* an
    // existing ref, so either can re-mode what the other made (`prova.reads(prova.port(5432))`).
    // `port` is exclusive — a listener is a writer of its port — and `reads` can widen it.
    prova.set(
        "port",
        lua.create_function(|lua, number: u64| {
            lua.create_userdata(ResourceRef {
                token: format!("port:{number}"),
                shared: false,
            })
        })?,
    )?;
    prova.set(
        "writes",
        lua.create_function(|lua, v: Value| resource_ref(lua, v, false))?,
    )?;
    prova.set(
        "reads",
        lua.create_function(|lua, v: Value| resource_ref(lua, v, true))?,
    )?;
    // The pre-`reads`/`writes` spellings, kept working but deliberately unadvertised: `resource` ==
    // `writes`, `shared` == `reads`. Their stubs are `---@deprecated`, which keeps an existing suite
    // resolving in the IDE while hiding them from `prova.help` — so nothing points a new author at
    // them, and no one's tests break the day they upgrade.
    prova.set(
        "resource",
        lua.create_function(|lua, v: Value| resource_ref(lua, v, false))?,
    )?;
    prova.set(
        "shared",
        lua.create_function(|lua, v: Value| resource_ref(lua, v, true))?,
    )?;

    // The host port mode, readable by topology/plugin authors as `prova.ports` (`"auto"` | `"fixed"`).
    // `prova.containerized` consults it to upgrade random ports to fixed bindings under `--fixed`; a
    // recipe with an advertised listener (Kafka) reads it to emit the right listener address.
    prova.set("ports", config.ports.as_str())?;

    // Where the project is (`RunConfig::with_project`) — so a repo-local plugin can say
    // `prova.root .. "/target/debug/miniond"` instead of hardcoding an absolute path or trusting the
    // process cwd. `prova.root` and `prova.home` are synonyms for the project ROOT. Absent (nil) when
    // there is no manifest, e.g. a bare `prova <file>` run.
    if let Some(dir) = &config.project_dir {
        let dir = dir.to_string_lossy();
        prova.set("root", dir.as_ref())?;
        prova.set("home", dir.as_ref())?;
    }

    // `prova.help([filter])` — the API surface, discoverable from inside the environment being
    // driven. Returns DATA (a list of `{name, signature, summary}`), not printed prose, so an agent
    // can filter it and a proof can assert on it. Parsed from the same LuaCATS stubs that ship to
    // the IDE — one source, two sinks. See `help.rs` / docs/design/agent-ergonomics.md §0.
    let help_roots = config.help_roots.clone();
    prova.set(
        "help",
        lua.create_function(move |lua, filter: Option<String>| {
            let all =
                crate::help::entries_with_plugins(help_roots.iter().map(|p| p.as_path()));
            let entries = match filter.as_deref().map(str::trim) {
                Some(n) if !n.is_empty() => crate::help::filter(&all, n),
                _ => all,
            };
            let out = lua.create_table()?;
            for (i, e) in entries.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("name", e.name.as_str())?;
                row.set("signature", e.signature.as_str())?;
                row.set("summary", e.summary.as_str())?;
                out.set(i + 1, row)?;
            }
            Ok(out)
        })?,
    )?;

    lua.globals().set("prova", prova)?;

    // `runtime.*` — the companion's config DSL — is NOT available in a test/eval/topology state.
    // Accessing ANY member here raises a clear error instead of a baffling nil, because `runtime`
    // configures the environment tests run *in*, and only `prova.lua` loads early enough (with the
    // manifest, before any test) to do that. `load_project_config` overwrites this stub with the
    // working table when it loads the companion. Keeping it off `prova` — the authoring surface — is
    // what makes the boundary self-evident.
    {
        let stub = lua.create_table()?;
        let mt = lua.create_table()?;
        mt.set(
            "__index",
            lua.create_function(|_, (_t, key): (Table, String)| {
                Err::<mlua::Value, _>(mlua::Error::RuntimeError(format!(
                    "runtime.{key} is only available in prova.lua (the project companion), not in a \
                     test — the runtime config DSL loads with the manifest, before any test runs"
                )))
            })?,
        )?;
        stub.set_metatable(Some(mt))?;
        lua.globals().set("runtime", stub)?;
    }

    // The typed fixture-scope constants: `Scope.Test` / `Scope.Flow` / `Scope.File` / `Scope.Suite`.
    lua.globals().set("Scope", make_scope_global(&lua)?)?;

    // `suite.config{ name?, requires? }` — configure the current suite (used in a `suite.lua`
    // setup file). `requires` gates the whole suite: it folds into the root node so every test
    // inherits it, and an unmet capability skips all the suite's files cleanly (skip, not fail).
    // `spec` is deliberately NOT accepted here: spec flags are test-level only — a suite-wide
    // flag recreates the graduation ceremony the revised design removed (api-freeze §5).
    {
        let col = col.clone();
        let suite = lua.create_table()?;
        suite.set(
            "config",
            lua.create_function(move |_, opts: Table| {
                let mut c = col.borrow_mut();
                if let Some(name) = opts.get::<Option<String>>("name")? {
                    c.nodes[0].name = name;
                }
                if let Some(reqs) = opts.get::<Option<Vec<String>>>("requires")? {
                    c.nodes[0].opts.requires.extend(reqs);
                }
                if !matches!(opts.get::<Value>("spec")?, Value::Nil) {
                    return Err(mlua::Error::RuntimeError(
                        "spec is test-level only — flag each open test, not the suite".into(),
                    ));
                }
                if !matches!(opts.get::<Value>("proves")?, Value::Nil) {
                    return Err(mlua::Error::RuntimeError(
                        "proves is test-level only — annotate each test, not the suite".into(),
                    ));
                }
                Ok(())
            })?,
        )?;
        lua.globals().set("suite", suite)?;
    }

    // First-party capability modules (`shell`, `fs`) as their own injected globals.
    crate::modules::install(&lua)?;

    // Host-provided plugin modules (e.g. `archetect`), installed into every Lua state.
    for install in &config.modules {
        install(&lua)?;
    }

    // Wire `require` to resolve Lua plugins (bundled + manifest + disk). Installed last so a plugin
    // loaded via `require` sees every primitive global it composes.
    //
    // Search roots are exactly what the embedder declared (`with_plugin_root`) — the engine adds
    // none of its own. It used to join `<project_root>/.prova/plugins` here, which meant the answer
    // to "where do plugins come from?" was split between this file and the manifest. The CLI now
    // passes the manifest's `[run] plugin_root` (already absolutised against the project root), so
    // the manifest is the single, readable source of truth and the engine has no layout opinion.
    crate::plugins::install(
        &lua,
        &config.plugin_roots,
        &config.named_plugins,
        &config.plugin_namespaces,
    )?;

    Ok((lua, col))
}

struct GroupBuilder {
    col: SharedCollector,
    ix: NodeIx,
}

impl UserData for GroupBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("test", |lua, this, (name, a, b): (String, Value, Value)| {
            let line = caller_line(lua, &this.col);
            let ix = register_test(&this.col, this.ix, name, a, b, None, line)?;
            lua.create_userdata(UnitHandle { ix })
        });

        methods.add_method(
            "test_each",
            |lua, this, (name, cases, factory): (String, Table, Function)| {
                register_test_each(lua, &this.col, this.ix, name, cases, factory)
            },
        );

        methods.add_method(
            "group",
            |lua, this, (name, a, b): (String, Value, Value)| {
                let ix = register_group(lua, &this.col, this.ix, name, a, b)?;
                lua.create_userdata(UnitHandle { ix })
            },
        );

        methods.add_method("flow", |lua, this, (name, a, b): (String, Value, Value)| {
            let ix = register_flow(lua, &this.col, this.ix, name, a, b)?;
            lua.create_userdata(UnitHandle { ix })
        });

        // Label-only subgrouping: structurally a nested group whose builder body nests explicitly
        // via `g:test`/etc. (inside a group you use the builder, so no ambient stack is needed here).
        methods.add_method(
            "describe",
            |lua, this, (label, body): (String, Function)| {
                register_group(
                    lua,
                    &this.col,
                    this.ix,
                    label,
                    Value::Function(body),
                    Value::Nil,
                )?;
                Ok(())
            },
        );
    }
}

/// The line of the innermost Lua stack frame that lives in the file currently being collected —
/// i.e. the call site of the `prova.test`/`group`/`flow`/`step` declaration executing right now.
///
/// Chunks are loaded with `set_name("@<file path>")` (see `file_chunk_name`), so a frame belongs
/// to the current file exactly when its debug source — prefix stripped — equals the collector's
/// `file_paths[current_file]`. Walking until that match (rather than taking the innermost Lua
/// frame) attributes a declaration made *through a helper* to the test file's call site, not the
/// helper's body. `None` when nothing matches (an `eval` snippet, a topology chunk, or a
/// declaration driven entirely from foreign code).
fn caller_line(lua: &Lua, col: &SharedCollector) -> Option<u32> {
    let expect: Option<String> = {
        let c = col.borrow();
        c.file_paths
            .get(c.current_file)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().into_owned())
    };
    for level in 0..=16 {
        let frame = lua.inspect_stack(level, |d| {
            let src = d.source().source.map(|s| s.into_owned());
            (src, d.current_line())
        })?; // past the top of the stack — no matching frame
        let (Some(src), Some(line)) = frame else {
            continue; // a C frame, or a frame with no line info
        };
        // Strip Lua's chunk-name prefixes: '@' marks a file source (ours), '=' a synthetic one.
        let src = src.strip_prefix(['@', '=']).unwrap_or(&src);
        match &expect {
            Some(e) if src == e => return Some(line as u32),
            Some(_) => continue,
            None => return Some(line as u32), // no file to match — take the innermost Lua frame
        }
    }
    None
}

/// Register a leaf `test`/`step` node under `parent`; returns its arena index (the unit handle id).
/// `case` is the `test_each` case value (`None` for an ordinary test); `line` is the declaration
/// call site (see `caller_line`), shared across every case of a `test_each`.
fn register_test(
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
    case: Option<Value>,
    line: Option<u32>,
) -> mlua::Result<NodeIx> {
    let (opts, body) = split_opts_body(a, b)?;
    Ok(col.borrow_mut().add(
        parent,
        Node {
            name,
            kind: NodeKind::Test,
            params: Params::default(),
            opts,
            children: vec![],
            body: Some(body),
            case,
            file: 0,
            line,
        },
    ))
}

/// Register one `test` per entry in `cases` (a 1-based sequence of case tables), all sharing the
/// same `factory` body. Each generated test carries its own case (delivered as the body's second
/// argument and as `t.case`), and its name is `name_template` with `{key}` placeholders filled from
/// the case. Returns a sequence of the generated unit handles (usable in `depends_on`).
fn register_test_each(
    lua: &Lua,
    col: &SharedCollector,
    parent: NodeIx,
    name_template: String,
    cases: Table,
    factory: Function,
) -> mlua::Result<Table> {
    let line = caller_line(lua, col); // the one test_each call site, shared by every case
    let handles = lua.create_table()?;
    for i in 1..=cases.raw_len() {
        let case: Value = cases.get(i)?;
        let name = render_case_name(&name_template, &case)?;
        let ix = register_test(
            col,
            parent,
            name,
            Value::Function(factory.clone()),
            Value::Nil,
            Some(case),
            line,
        )?;
        handles.push(lua.create_userdata(UnitHandle { ix })?)?;
    }
    Ok(handles)
}

/// Fill `{key}` placeholders in a `test_each` name template from the case table. An unknown key (or a
/// non-table case) leaves the `{key}` literal in place rather than failing — the name is cosmetic.
fn render_case_name(template: &str, case: &Value) -> mlua::Result<String> {
    let tbl = match case {
        Value::Table(t) => Some(t.clone()),
        _ => None,
    };
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                let replaced = match &tbl {
                    Some(t) => match t.get::<Value>(key)? {
                        Value::Nil => format!("{{{key}}}"),
                        other => value_to_string(&other),
                    },
                    None => format!("{{{key}}}"),
                };
                out.push_str(&replaced);
                rest = &after[close + 1..];
            }
            None => {
                // Unbalanced brace: emit the rest verbatim.
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// A scalar Lua value rendered for a test name. Non-scalars (tables/functions) are unlikely in a name
/// placeholder; render them as `?` rather than erroring.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string_lossy().to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Nil => String::new(),
        _ => "?".to_string(),
    }
}

/// Register a `group` node under `parent` and run its builder body to collect child units.
/// Accepts `(name, body)` or `(name, opts, body)`; `opts.depends_on` gates the whole group.
fn register_group(
    lua: &Lua,
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
) -> mlua::Result<NodeIx> {
    let (opts, body) = split_opts_body(a, b)?;
    let line = caller_line(lua, col);
    let gix = col.borrow_mut().add(
        parent,
        Node {
            name,
            kind: NodeKind::Group,
            params: Params::default(),
            opts,
            children: vec![],
            body: None,
            case: None,
            file: 0,
            line,
        },
    );
    let gb = lua.create_userdata(GroupBuilder {
        col: col.clone(),
        ix: gix,
    })?;
    col.borrow_mut().builder_depth += 1;
    let ran = body.call::<()>(gb);
    col.borrow_mut().builder_depth -= 1;
    ran?;
    let c = col.borrow();
    if c.nodes[gix].children.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "group {:?} declared no children — declare them on the builder argument \
             (`function(g) g:test(name, fn) end`)",
            c.nodes[gix].name
        )));
    }
    drop(c);
    Ok(gix)
}

/// Register a `describe` labeling group under the current ambient parent, then run its body with
/// that group pushed on the parent stack so **bare** `prova.test`/`test_each`/`group`/`flow` inside
/// the body nest under the label (dynamic scoping). Structurally a group — labeling only, no new
/// fixture scope. The stack is popped even if the body errors, so one bad `describe` can't corrupt
/// the ambient parent for the rest of the file.
fn register_describe(
    lua: &Lua,
    col: &SharedCollector,
    label: String,
    body: Function,
) -> mlua::Result<()> {
    let line = caller_line(lua, col);
    let ix = {
        let mut c = col.borrow_mut();
        let parent = c.current_parent();
        c.add(
            parent,
            Node {
                name: label,
                kind: NodeKind::Group,
                params: Params::default(),
                opts: UnitOpts::default(),
                children: vec![],
                body: None,
                case: None,
                file: 0,
                line,
            },
        )
    };
    col.borrow_mut().parent_stack.push(ix);
    let result = body.call::<()>(());
    col.borrow_mut().parent_stack.pop();
    result
}

/// Register a `flow` node under `parent` and run its builder body to collect the ordered steps.
/// Accepts `(name, body)` or `(name, opts, body)`. The body runs once at collection time; its
/// closures share upvalues (the flow's context bag), so `local x` captured across steps is
/// genuinely shared state — the flow's one blessed way to carry built-up context, which a `group`
/// structurally cannot express.
fn register_flow(
    lua: &Lua,
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
) -> mlua::Result<NodeIx> {
    let (opts, body) = split_opts_body(a, b)?;
    let line = caller_line(lua, col);
    let fix = col.borrow_mut().add(
        parent,
        Node {
            name,
            kind: NodeKind::Flow,
            params: Params::default(),
            opts,
            children: vec![],
            body: None,
            case: None,
            file: 0,
            line,
        },
    );
    let fb = lua.create_userdata(FlowBuilder {
        col: col.clone(),
        ix: fix,
    })?;
    col.borrow_mut().builder_depth += 1;
    let ran = body.call::<()>(fb);
    col.borrow_mut().builder_depth -= 1;
    ran?;
    let c = col.borrow();
    if c.nodes[fix].children.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "flow {:?} declared no steps — declare them on the builder argument \
             (`function(flow) flow:step(name, fn) end`)",
            c.nodes[fix].name
        )));
    }
    drop(c);
    Ok(fix)
}

/// Builds a flow's ordered steps. Only exposes `step` — no nested groups, no unordered children —
/// because a flow's contract is *sequence*. Shared context is carried by closure upvalues, so the
/// builder needs no state-bag method.
struct FlowBuilder {
    col: SharedCollector,
    ix: NodeIx,
}

impl UserData for FlowBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("step", |lua, this, (name, a, b): (String, Value, Value)| {
            let (opts, body) = split_opts_body(a, b)?;
            let line = caller_line(lua, &this.col);
            this.col.borrow_mut().add(
                this.ix,
                Node {
                    name,
                    kind: NodeKind::Test,
                    params: Params::default(),
                    opts,
                    children: vec![],
                    body: Some(body),
                    case: None,
                    file: 0,
                    line,
                },
            );
            Ok(())
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Plan (definition → plan → execute)
// ---------------------------------------------------------------------------------------------

struct PlanItem {
    path: String,
    body: Function,
    timeout: Option<Duration>,
    case: Option<Value>,
    /// Source file index — selects this item's `Scope.File` instance.
    file: usize,
    /// Declaration line in that file (captured at registration), for reported source locations.
    line: Option<u32>,
}

/// A scheduling atom. Independent units may run concurrently (`buffer_unordered`); a flow's steps
/// are serial *within* the unit but the flow parallelizes with its siblings like any other unit.
enum PlanUnit {
    Test(PlanItem),
    Flow { steps: Vec<PlanItem> },
}

impl PlanUnit {
    /// Every reported leaf path in this unit (a test is one; a flow is one per step).
    fn leaf_paths(&self) -> Vec<&str> {
        match self {
            PlanUnit::Test(item) => vec![item.path.as_str()],
            PlanUnit::Flow { steps } => steps.iter().map(|s| s.path.as_str()).collect(),
        }
    }

    /// Every reported leaf item in this unit — `leaf_paths` with the whole item, for callers that
    /// also need the source location.
    fn items(&self) -> Vec<&PlanItem> {
        match self {
            PlanUnit::Test(item) => vec![item],
            PlanUnit::Flow { steps } => steps.iter().collect(),
        }
    }
}

fn plan_item(node: &Node, ancestors: &[String]) -> PlanItem {
    let mut path = ancestors.to_vec();
    path.push(format!("{}{}", node.name, node.params.suffix()));
    PlanItem {
        path: path.join(" › "),
        body: node.body.clone().expect("test/step node has a body"),
        timeout: node.opts.timeout,
        case: node.case.clone(),
        file: node.file,
        line: node.line,
    }
}

/// One schedulable unit: a top-level `test` or a `flow` (a group is not a leaf — it expands to the
/// leaves under it). `deps` are the leaf ids this leaf must wait for; a leaf is skipped if any of
/// them failed or was skipped. `deps` and `reqs` already fold in **inherited** group-level options.
struct Leaf {
    unit: PlanUnit,
    /// Node-level dependencies (own `depends_on` + inherited from ancestor groups), pre-expansion.
    raw_deps: Vec<NodeIx>,
    /// Expanded leaf-id dependencies (filled by `expand_deps`).
    deps: Vec<usize>,
    /// Resources this leaf holds while running (own + inherited; plus the injected global for
    /// `serial`). The scheduler will not co-schedule two leaves whose reqs conflict.
    reqs: Vec<ResourceReq>,
    /// Process-wide exclusive (never concurrent with anything).
    serial: bool,
    /// Capabilities this leaf needs (own + inherited); resolved into `precondition_skip`.
    requires: Vec<String>,
    /// If set, this leaf is skipped before it ever runs (an unmet `requires`), with this reason.
    precondition_skip: Option<String>,
    /// Effective tags: the unit's own plus every enclosing group's (selection matches on these).
    tags: Vec<String>,
    /// `Some(reason)` when this leaf carries its own `spec` flag (always a non-empty reason)
    /// — test-level only, never inherited. Drives the outcome inversion: red body →
    /// `Outcome::Spec`, green body → a failure demanding the flag's removal.
    spec: Option<String>,
}

/// Group-level options that flow down to every contained leaf: `depends_on`, `resources`, `serial`,
/// `requires`.
#[derive(Clone, Default)]
struct Inherited {
    deps: Vec<NodeIx>,
    resources: Vec<ResourceReq>,
    serial: bool,
    requires: Vec<String>,
    tags: Vec<String>,
}

/// The executable plan: a flat list of leaves plus the leaf-level dependency DAG.
struct Plan {
    leaves: Vec<Leaf>,
}

/// Walk the tree, emitting a `Leaf` per test/flow and recording, for every node, which leaves live
/// under it (so a `depends_on`/`resources` on a group can expand to that group's leaves).
/// `inherited` carries ancestor groups' options down so a group-level declaration applies to each
/// contained leaf.
fn collect_leaves(
    col: &Collector,
    ix: NodeIx,
    ancestors: &mut Vec<String>,
    inherited: &Inherited,
    leaves: &mut Vec<Leaf>,
    node_leaves: &mut HashMap<NodeIx, Vec<usize>>,
) -> Vec<usize> {
    let node = &col.nodes[ix];
    let my_leaves = match node.kind {
        NodeKind::Group => {
            let named = ix != 0 && !node.name.is_empty();
            if named {
                ancestors.push(format!("{}{}", node.name, node.params.suffix()));
            }
            let mut child_inherited = inherited.clone();
            child_inherited
                .deps
                .extend(node.opts.depends_on.iter().copied());
            child_inherited
                .resources
                .extend(node.opts.resources.iter().cloned());
            child_inherited.serial |= node.opts.serial;
            child_inherited
                .requires
                .extend(node.opts.requires.iter().cloned());
            child_inherited.tags.extend(node.opts.tags.iter().cloned());
            let mut ids = Vec::new();
            for &child in &node.children {
                ids.extend(collect_leaves(
                    col,
                    child,
                    ancestors,
                    &child_inherited,
                    leaves,
                    node_leaves,
                ));
            }
            if named {
                ancestors.pop();
            }
            ids
        }
        NodeKind::Flow => {
            ancestors.push(format!("{}{}", node.name, node.params.suffix()));
            let steps = node
                .children
                .iter()
                .map(|&c| plan_item(&col.nodes[c], ancestors))
                .collect();
            ancestors.pop();
            let id = push_leaf(leaves, PlanUnit::Flow { steps }, node, inherited);
            vec![id]
        }
        NodeKind::Test => {
            let id = push_leaf(
                leaves,
                PlanUnit::Test(plan_item(node, ancestors)),
                node,
                inherited,
            );
            vec![id]
        }
    };
    node_leaves.insert(ix, my_leaves.clone());
    my_leaves
}

fn push_leaf(leaves: &mut Vec<Leaf>, unit: PlanUnit, node: &Node, inherited: &Inherited) -> usize {
    let mut raw_deps = inherited.deps.clone();
    raw_deps.extend(node.opts.depends_on.iter().copied());
    let mut reqs = inherited.resources.clone();
    reqs.extend(node.opts.resources.iter().cloned());
    let mut requires = inherited.requires.clone();
    requires.extend(node.opts.requires.iter().cloned());
    let mut tags = inherited.tags.clone();
    tags.extend(node.opts.tags.iter().cloned());
    let id = leaves.len();
    leaves.push(Leaf {
        unit,
        raw_deps,
        deps: Vec::new(),
        reqs,
        serial: inherited.serial || node.opts.serial,
        requires,
        precondition_skip: None,
        tags,
        // Test-level only, by design: the leaf's own flag, never an ancestor's.
        spec: node.opts.spec.clone(),
    });
    id
}

/// Narrow a plan to the selection, pulling in the dependencies of every selected leaf (an outcome
/// gate can't be evaluated against a node that never ran) and remapping leaf-id edges. Returns the
/// surviving plan and how many leaves were deselected. Flows are atomic: a flow is selected if ANY
/// of its step paths match.
fn apply_selection(plan: Plan, sel: &Selection) -> (Plan, usize) {
    if sel.is_empty() {
        return (plan, 0);
    }
    let mut keep = vec![false; plan.leaves.len()];
    for (i, leaf) in plan.leaves.iter().enumerate() {
        if sel.selects(&leaf.unit.leaf_paths(), &leaf.tags) {
            keep[i] = true;
        }
    }
    narrow_plan(plan, keep)
}

/// Narrow a plan to the leaves carrying an effective spec flag (`--specs`): the burndown
/// selector. Graduated leaves are ordinary tests again and count as deselected, exactly like an
/// unmatched `-k` — the spec surface is precisely what is still open (or wrongly green).
fn apply_specs_filter(plan: Plan, enabled: bool) -> (Plan, usize) {
    if !enabled {
        return (plan, 0);
    }
    let keep = plan.leaves.iter().map(|l| l.spec.is_some()).collect();
    narrow_plan(plan, keep)
}

/// Keep exactly the marked leaves plus the dependency closure of every one of them (an outcome
/// gate can't be evaluated against a node that never ran), remapping leaf-id edges. Returns the
/// surviving plan and how many leaves were dropped.
fn narrow_plan(plan: Plan, mut keep: Vec<bool>) -> (Plan, usize) {
    // Dependency closure: selected leaves drag their upstream gates in, transitively.
    let mut work: Vec<usize> = keep
        .iter()
        .enumerate()
        .filter_map(|(i, &k)| k.then_some(i))
        .collect();
    while let Some(i) = work.pop() {
        for &d in &plan.leaves[i].deps {
            if !keep[d] {
                keep[d] = true;
                work.push(d);
            }
        }
    }
    let deselected = keep.iter().filter(|&&k| !k).count();
    if deselected == 0 {
        return (plan, 0);
    }
    let mut remap = vec![usize::MAX; plan.leaves.len()];
    let mut kept = Vec::with_capacity(plan.leaves.len() - deselected);
    for (i, leaf) in plan.leaves.into_iter().enumerate() {
        if keep[i] {
            remap[i] = kept.len();
            kept.push(leaf);
        }
    }
    for leaf in &mut kept {
        leaf.deps = leaf.deps.iter().map(|&d| remap[d]).collect();
    }
    (Plan { leaves: kept }, deselected)
}

/// Turn each leaf's node-level `raw_deps` (which may point at a group) into concrete leaf-id edges,
/// dropping any self-edge.
fn expand_deps(leaves: &mut [Leaf], node_leaves: &HashMap<NodeIx, Vec<usize>>) {
    for (i, leaf) in leaves.iter_mut().enumerate() {
        let raw = std::mem::take(&mut leaf.raw_deps);
        let mut set = std::collections::BTreeSet::new();
        for dep_ix in raw {
            if let Some(dep_leaves) = node_leaves.get(&dep_ix) {
                for &dl in dep_leaves {
                    if dl != i {
                        set.insert(dl);
                    }
                }
            }
        }
        leaf.deps = set.into_iter().collect();
    }
}

/// Kahn-style reachability over the leaf DAG. Returns the names of leaves caught in a cycle, if any.
/// (Handle references are backward in definition order, so a cycle is practically unreachable from
/// valid Lua — this is a defensive collection-time guard the design mandates.)
fn find_cycle(leaves: &[Leaf]) -> Option<Vec<String>> {
    let n = leaves.len();
    let mut resolved = vec![false; n];
    let mut remaining = n;
    while remaining > 0 {
        let mut progressed = false;
        for i in 0..n {
            if resolved[i] {
                continue;
            }
            if leaves[i].deps.iter().all(|&d| resolved[d]) {
                resolved[i] = true;
                remaining -= 1;
                progressed = true;
            }
        }
        if !progressed {
            return Some(
                (0..n)
                    .filter(|&i| !resolved[i])
                    .map(|i| unit_name(&leaves[i]).to_string())
                    .collect(),
            );
        }
    }
    None
}

/// A leaf's display name for messages — its first reported path (the flow/test name with ancestry).
fn unit_name(leaf: &Leaf) -> &str {
    leaf.unit.leaf_paths().first().copied().unwrap_or("<unit>")
}

/// The reserved token that makes `serial` work: a serial leaf takes it exclusively while every
/// other leaf takes it shared, so RW semantics alone enforce "never concurrent with anything".
const SERIAL_TOKEN: &str = "__prova_serial__";

fn build_plan(col: &Collector, caps: &Capabilities) -> mlua::Result<Plan> {
    // Spec flags and proves attributes are test-level only (api-freeze §5, revised): either on a
    // group would need the whole inheritance/graduation ceremony back. Refuse with the fix.
    for node in &col.nodes {
        if !matches!(node.kind, NodeKind::Group) {
            continue;
        }
        let name = if node.name.is_empty() {
            "the suite".to_string()
        } else {
            format!("group {:?}", node.name)
        };
        if node.opts.spec.is_some() {
            return Err(mlua::Error::RuntimeError(format!(
                "spec is test-level only — flag each open test, not {name}"
            )));
        }
        if node.opts.proves.is_some() {
            return Err(mlua::Error::RuntimeError(format!(
                "proves is test-level only — annotate each test, not {name}"
            )));
        }
    }
    let mut leaves = Vec::new();
    let mut node_leaves = HashMap::new();
    collect_leaves(
        col,
        0,
        &mut Vec::new(),
        &Inherited::default(),
        &mut leaves,
        &mut node_leaves,
    );
    expand_deps(&mut leaves, &node_leaves);
    if let Some(cycle) = find_cycle(&leaves) {
        return Err(mlua::Error::RuntimeError(format!(
            "dependency cycle detected among units: {}",
            cycle.join(", ")
        )));
    }
    // Only pay the global-token cost when someone actually asked for serial execution. A serial
    // leaf holds it exclusively; everyone else reads it, so a serial leaf waits for all others to
    // drain and blocks new starts — exactly "process-wide exclusive".
    if leaves.iter().any(|l| l.serial) {
        for leaf in &mut leaves {
            leaf.reqs.push(ResourceReq {
                token: SERIAL_TOKEN.to_string(),
                shared: !leaf.serial,
            });
        }
    }
    // Resolve `requires`: a leaf with an unavailable capability is pre-skipped (not failed).
    // Detect each distinct capability once — some detectors shell out (e.g. `docker info`).
    resolve_requires(&mut leaves, caps);
    Ok(Plan { leaves })
}

/// Set `precondition_skip` on any leaf whose `requires` are not satisfied.
///
/// A capability is an expression, not just a name: `"docker"` or `"dotnet >= 9"`. The skip reason
/// distinguishes the three ways it can go unmet, because they call for different actions — install
/// the tool, upgrade it, or fix the typo:
///
/// - **absent**    → "requires \"docker\" (unavailable)"
/// - **too old**   → "requires \"dotnet >= 9\" (dotnet 8.0.421 does not satisfy >= 9)"
/// - **malformed** → an error, not a skip: a constraint that can never parse would skip forever
///   and read as green, which is the vacuous green this contract exists to remove.
fn resolve_requires(leaves: &mut [Leaf], caps: &Capabilities) {
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    for leaf in leaves.iter_mut() {
        for cap in &leaf.requires {
            // `None` = satisfied; `Some(reason)` = not, and why. Memoized: version probes shell out.
            let unmet = cache
                .entry(cap.clone())
                .or_insert_with(|| caps.unmet_reason(cap))
                .clone();
            if let Some(reason) = unmet {
                leaf.precondition_skip = Some(format!("skipped: {reason}"));
                break;
            }
        }
    }
}

/// Capabilities the project registered in its `prova.lua` companion — name → its reported version
/// (`None` = available but versionless).
///
/// **Per run, not global.** This lives in [`RunConfig`], so two projects resolved in one process —
/// the warm MCP resolving one at startup, then `run { project }` — cannot see each other's
/// vocabulary. It was a process-global static once, and that leaked across projects
/// (`tests/capability_isolation.rs`).
///
/// **Answers, not closures, evaluated once at load.** Three reasons that are one: `must_run` is a
/// precondition checked before any suite exists (nothing to call back into); each suite gets its own
/// `Lua` and mlua handles are `!Send` (a stored closure could not cross states); and a capability
/// that answered differently for two suites in one run would be a coin flip, not a capability.
#[derive(Clone, Default, Debug)]
pub struct Capabilities(std::collections::BTreeMap<String, Option<semver::Version>>);

impl Capabilities {
    /// Record a registered capability. `version` is `None` for a bare `true` predicate (available,
    /// no version to compare).
    pub fn register(&mut self, name: &str, version: Option<semver::Version>) {
        self.0.insert(name.to_string(), version);
    }

    /// Available = registered by the project, OR a built-in the host provides. Registered wins, but
    /// registering over a built-in is refused at load, so this cannot shadow `docker`.
    pub fn available(&self, name: &str) -> bool {
        self.0.contains_key(name) || builtin_available(name)
    }

    /// The version a constraint compares against: the project's reported version if registered, else
    /// a probed built-in version. `None` = no version to compare, which makes a constraint
    /// unsatisfiable — "cannot confirm" is not "satisfied".
    pub fn version(&self, name: &str) -> Option<semver::Version> {
        if let Some(v) = self.0.get(name) {
            return v.clone();
        }
        builtin_version(name)
    }

    /// Is this capability expression satisfied here, and if not, why?
    ///
    /// - `Ok(None)`         — satisfied.
    /// - `Ok(Some(reason))` — unmet: absent, or the wrong version (phrased for a human).
    /// - `Err(e)`           — the expression is malformed (a config error, not an environment one).
    ///
    /// The one function both halves of the contract call — `requires` (skip on unmet) and `must_run`
    /// (fail on unmet) — so they can never disagree about what a string means. Name before version,
    /// so an absent tool never reaches a probe and `windows >= 10` short-circuits on unix.
    pub fn expr_status(&self, expr: &str) -> Result<Option<String>, String> {
        let parsed = CapabilityExpr::parse(expr)?;
        if !self.available(parsed.name) {
            return Ok(Some(format!("{:?} is unavailable", parsed.name)));
        }
        let Some(req) = parsed.req else {
            return Ok(None);
        };
        match self.version(parsed.name) {
            Some(v) if req.matches(&v) => Ok(None),
            Some(v) => Ok(Some(format!("{} {v} does not satisfy {req}", parsed.name))),
            None => Ok(Some(format!(
                "{}'s version could not be determined, so {req} cannot be confirmed",
                parsed.name
            ))),
        }
    }

    /// Why `expr` is unmet, or `None` if satisfied — the skip-side phrasing over `expr_status`. A
    /// malformed expression is reported as the reason rather than folded into "absent": the author
    /// needs to see the typo, not hunt for a tool that was never named.
    fn unmet_reason(&self, expr: &str) -> Option<String> {
        match self.expr_status(expr) {
            Ok(None) => None,
            Ok(Some(reason)) => Some(format!("requires {expr:?} ({reason})")),
            Err(e) => Some(e),
        }
    }
}

/// Is `name` a capability this build defines itself? Registering over one is refused: `docker` means
/// something specific (a daemon that answers AND runs linux containers), and letting a project
/// redefine it would make `requires = { "docker" }` mean different things in different repos —
/// silently, which is the worst kind.
pub fn is_builtin_capability(name: &str) -> bool {
    matches!(
        name,
        "docker" | "github" | "network" | "internet" | "unix" | "windows"
    ) || native_capability_compiled(name).is_some()
}

/// A capability expression: a name, optionally with a semver constraint — `"docker"`,
/// `"dotnet >= 9"`, `"node ^20"`, `"git >= 1.0, < 3.0"`.
///
/// It is a **string**, and that is load-bearing rather than lazy. `must_run` lives in `prova.toml`,
/// which is TOML and holds no functions, so a predicate expressible only in Lua would split the
/// contract into two vocabularies — one for what a test needs, another for what a context
/// guarantees. One string parses for both.
pub struct CapabilityExpr<'a> {
    pub name: &'a str,
    pub req: Option<semver::VersionReq>,
}

impl<'a> CapabilityExpr<'a> {
    /// Parse `"<name>"` or `"<name> <constraint>"`. An unparseable constraint is an **error**, never
    /// a quiet "unavailable": a typo'd constraint that silently never matched would skip forever and
    /// read as green — the vacuous green this whole contract exists to remove.
    pub fn parse(expr: &'a str) -> Result<Self, String> {
        let expr = expr.trim();
        // The name runs until whitespace or the first constraint character, so `git>=1.0` and
        // `git >= 1.0` are the same expression — whitespace is not meaning.
        let split = expr
            .find(|c: char| c.is_whitespace() || "<>=^~".contains(c))
            .unwrap_or(expr.len());
        let (name, rest) = expr.split_at(split);
        let name = name.trim();
        let rest = rest.trim();
        if name.is_empty() {
            return Err(format!("invalid capability expression {expr:?}: no name"));
        }
        if rest.is_empty() {
            return Ok(Self { name, req: None });
        }
        match semver::VersionReq::parse(rest) {
            Ok(req) => Ok(Self {
                name,
                req: Some(req),
            }),
            Err(e) => Err(format!(
                "invalid capability expression {expr:?}: {e} \
                 (expected a semver constraint like \">= 9\", \"^20\", or \">= 1.0, < 3.0\")"
            )),
        }
    }
}

/// What version of `name` is installed, if the question is meaningful and answerable.
///
/// `None` means "no version to compare" — either the capability has no version concept, or its probe
/// could not answer. A constraint against `None` is **unsatisfiable**, because the honest response to
/// "is this ≥ 9?" when the version is unknowable is "cannot confirm", and a gate that cannot confirm
/// must not wave the suite through.
fn builtin_version(name: &str) -> Option<semver::Version> {
    let raw = match name {
        // Docker's SERVER version — the daemon is the thing a suite depends on, and it can differ
        // from the CLI talking to it. `docker --version` would report the client and quietly answer
        // a different question.
        "docker" => run_capture("docker", &["version", "--format", "{{.Server.Version}}"])?,
        // Platform predicates are booleans, not versions: `cfg!(unix)` has no number. A future
        // `windows >= 10` wants the OS build, which is a separate probe per platform; until that
        // exists, say so honestly (None ⇒ a constraint cannot be satisfied) rather than invent one.
        "unix" | "windows" => return None,
        // The general case: ask the tool. Every candidate answers `--version` on stdout —
        //   git    → "git version 2.54.0"
        //   dotnet → "8.0.421"
        //   sh     → "GNU bash, version 5.3.9(1)-release (…)"
        // so take the first version-shaped token rather than trying to know each tool's format.
        other => run_capture(other, &["--version"])?,
    };
    parse_first_version(&raw)
}

/// The first `N.N[.N]` in `text`, padded to three components.
///
/// Tools are inconsistent (`2.54`, `8.0.421`, `5.3.9(1)-release`) and an author should not have to
/// care, so normalize rather than demand strict semver from arbitrary CLIs.
fn parse_first_version(text: &str) -> Option<semver::Version> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let tok = text[start..i].trim_end_matches('.');
        let mut parts: Vec<&str> = tok.split('.').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 2 {
            parts.truncate(3);
            while parts.len() < 3 {
                parts.push("0");
            }
            if let Ok(v) = semver::Version::parse(&parts.join(".")) {
                return Some(v);
            }
        }
    }
    None
}

/// Run `program args…` and capture stdout, or `None` if it cannot be run.
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Is this capability expression satisfied here, and if not, why?
///
/// - `Ok(None)`         — satisfied.
/// - `Ok(Some(reason))` — unmet, phrased for a human: absent, or the wrong version.
/// - `Err(e)`           — the expression itself is malformed (a config error, not an environment one).
///
/// The three are kept apart because they call for different actions: install the tool, upgrade it,
/// or fix the typo. Name is checked before version, so an absent tool never reaches a version probe
/// and `windows >= 10` short-circuits on unix without asking Windows what build it is.
///
/// This is the one function both halves of the contract call — `requires` (skip on unmet) and
/// `must_run` (fail on unmet). They must never disagree about what a string means.
/// Is `name` a built-in capability this host provides? The base layer under [`Capabilities`]: a
/// project's registered names are consulted first (in `Capabilities::available`), then this answers
/// for `docker`, the platform predicates, compiled-in native clients, and finally any tool-of-that-
/// name on PATH (so `requires = { "kubectl" }` just works). A missing capability never fails a test
/// — it skips it, visibly.
fn builtin_available(name: &str) -> bool {
    match name {
        // The docker daemon must be reachable *and* the feature compiled in. Retry a few times: a
        // single `docker info` can transiently fail when the daemon is momentarily busy (heavy
        // container churn — e.g. many container tests tearing down at once), which would otherwise
        // skip a whole test spuriously. This resolves once per run (memoized), so the cost is bounded;
        // a genuinely-absent daemon fails fast (connection-refused is instant), so the retry budget is
        // paid mostly as backoff sleeps only when the daemon is present-but-busy.
        "docker" => cfg!(feature = "docker") && docker_runs_linux_containers(),
        "github" => std::env::var_os("GITHUB_TOKEN").is_some(),
        // Platform predicates. `shell.run("…")` routes a STRING through the platform's shell — `sh -c`
        // on unix, `cmd /C` on Windows — so a test asserting POSIX syntax (`$VAR`, `;`, `1>&2`,
        // `sleep`) genuinely *cannot run* off unix. That is a capability question, not a bug: the
        // honest answer is to skip, the way an absent docker daemon skips. (The argv form
        // `shell.run{"prog", "arg"}` needs no shell and stays portable — prefer it.)
        "unix" => cfg!(unix),
        "windows" => cfg!(windows),
        // No cheap, reliable synchronous probe; assume present (a real offline mode is future work).
        "network" | "internet" => true,
        // A native-client capability (`kafka`, `postgres`, …) is available iff its feature was
        // compiled into this build — so `requires = { "kafka" }` skips gracefully in a build that
        // lacks it, exactly as `docker` skips without a daemon. This is the unified gate: there is no
        // separate `requires_native`, just a capability with a compiled-in detector. Anything not a
        // native capability falls through to a tool-on-PATH probe (`requires = { "kubectl" }`).
        other => match native_capability_compiled(other) {
            Some(compiled) => compiled,
            None => binary_on_path(other),
        },
    }
}

/// Whether `name` is a native-client capability and, if so, whether *this* build compiled it in.
/// `Some(true)`/`Some(false)` for a known native capability; `None` if `name` is not one (so the
/// caller falls back to a binary-on-PATH probe). The name set is fixed (independent of features);
/// only the `cfg!` results vary per build, which is what makes a lean distribution skip cleanly.
fn native_capability_compiled(name: &str) -> Option<bool> {
    let compiled = match name {
        "http" => cfg!(feature = "http"),
        "sqlite" => cfg!(feature = "sqlite"),
        "grpc" => cfg!(feature = "grpc"),
        "graphql" => cfg!(feature = "graphql"),
        "yaml" => cfg!(feature = "yaml"),
        _ => return None,
    };
    Some(compiled)
}

/// Run `program args...`, discarding output; true iff it exits 0. Used for daemon-liveness checks.
fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `command_succeeds`, retried up to `attempts` times with a short backoff — for a daemon-liveness
/// probe that can hiccup transiently. Succeeds on the first passing attempt (so when the daemon is
/// healthy there is no delay); only a genuinely-absent daemon pays the full backoff.
/// Can this daemon run the **Linux** containers prova's resources are?
///
/// Answering `docker info` is not enough, and the gap is not hypothetical: Docker on Windows in
/// *Windows-container* mode answers `info` perfectly happily and then cannot pull
/// `postgres:16-alpine`. A suite that says `requires = { "docker" }` means "I am about to run a
/// linux image", so that is what the capability has to check — otherwise the gate waves the suite
/// through and it dies later on an obscure "Docker stream error" instead of skipping. Ask the daemon
/// what OS its containers are.
pub fn docker_runs_linux_containers() -> bool {
    if !command_succeeds_retry("docker", &["info"], 8) {
        return false;
    }
    std::process::Command::new("docker")
        .args(["info", "--format", "{{.OSType}}"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .eq_ignore_ascii_case("linux")
        })
        .unwrap_or(false)
}

fn command_succeeds_retry(program: &str, args: &[&str], attempts: u32) -> bool {
    for attempt in 0..attempts {
        if command_succeeds(program, args) {
            return true;
        }
        if attempt + 1 < attempts {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    false
}

/// Is an executable named `name` on `PATH`?
///
/// On Windows an executable on `PATH` carries an extension (`cargo.exe`), so probing the bare name
/// finds nothing and *every* `requires` gate would skip. Try each `PATHEXT` suffix as well —
/// `requires = { "cargo" }` names the tool, not the file.
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    let mut candidates = vec![name.to_string()];
    if cfg!(windows) {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        candidates.extend(
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| format!("{name}{ext}")),
        );
    }

    std::env::split_paths(&path).any(|dir| candidates.iter().any(|file| dir.join(file).is_file()))
}

struct NodeResult {
    path: String,
    outcome: Outcome,
    duration: Duration,
    assertions: usize,
    message: Option<String>,
    /// Source location of the declaration (file path + 1-based line), when the leaf has file
    /// backing — threaded into `Event::NodeFinished` for reporters.
    file: Option<String>,
    line: Option<u32>,
    /// True for a `⟶ teardown` leaf.
    ///
    /// Reported exactly like any other node — but it **never gates**. A cleanup that raised is not
    /// the work failing: the body already passed. So it must not cascade-skip a flow's later steps,
    /// and must not skip a `depends_on` dependent, either of which would report a defect in code
    /// that is fine. It is reported *because it happened*, not because the work failed. The flag
    /// makes that structural rather than positional — the first proof written here caught the
    /// alternative (keying on "any failed result") skipping a flow's remaining steps.
    teardown: bool,
    /// The spec flag's reason for an `Outcome::Spec` result (set by the inversion, threaded into
    /// `Event::NodeFinished::spec_reason`). `None` for every other outcome.
    spec: Option<String>,
}

/// The spec outcome inversion, applied to a spec-flagged leaf's results after it ran
/// (docs/plans/api-freeze.md §5). Teardown results are exempt — they report cleanup, not the work.
///
/// - Any work result **failed** → the leaf is an **open spec**: each failure becomes
///   `Outcome::Spec` (CI green) — unless `strict` (driver mode), where open specs stay failures.
/// - No failures and ≥1 pass → the spec is **honored**: each pass becomes a *failure* demanding
///   graduation — convert the flag to `proves = "<context>"` (preferred: the reason lives on in
///   the test) or remove it — so an implementation cannot land still flagged `spec`.
/// - All skipped → untouched: an unmet `requires` wins over spec (nothing was observed).
fn apply_spec_inversion(results: &mut [NodeResult], reason: &str, strict: bool) {
    let failed = results
        .iter()
        .any(|r| !r.teardown && r.outcome == Outcome::Failed);
    if failed {
        if !strict {
            for r in results
                .iter_mut()
                .filter(|r| !r.teardown && r.outcome == Outcome::Failed)
            {
                r.outcome = Outcome::Spec;
                r.spec = Some(reason.to_string());
            }
        }
        return;
    }
    // The graduation fix is copy-pasteable: the spec's (always non-empty) reason becomes the
    // proves context.
    let fix = format!("proves = {reason:?}");
    for r in results
        .iter_mut()
        .filter(|r| !r.teardown && r.outcome == Outcome::Passed)
    {
        r.outcome = Outcome::Failed;
        r.message = Some(format!(
            "spec honored — convert the spec flag to {fix} (keep the context) or remove it"
        ));
    }
}

/// Returns the test's own node, plus a `⟶ teardown` node per teardown failure (usually none).
async fn run_one(
    lua: &Lua,
    item: &PlanItem,
    state: &Rc<RunState>,
    flow_scope: Option<Rc<RefCell<ScopeState>>>,
) -> Vec<NodeResult> {
    let run = Rc::new(RefCell::new(TestRun::default()));
    // Snapshot context: where this test's `.snap` files live and how they're keyed. Absent when the
    // source file has no recorded path (e.g. a topology run), which makes `matches_snapshot` error.
    if let Some(dir) = state.snapshot_dir(item.file) {
        let stem = state
            .file_paths
            .get(item.file)
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("tests")
            .to_string();
        run.borrow_mut().snapshot = Some(SnapshotCtx {
            dir,
            stem,
            key_base: slugify(&item.path),
            update: state.update_snapshots,
            counter: 0,
            registry: state.snapshot_registry.clone(),
        });
    }
    let test_scope = Rc::new(RefCell::new(ScopeState::default()));
    // The case is delivered both as `t.case` and as the body's second argument, so `fn(t, case)`
    // and `fn(t)` (ignoring the trailing nil) both work.
    let case_arg = item.case.clone().unwrap_or(Value::Nil);
    let ctx = Ctx {
        run: run.clone(),
        state: state.clone(),
        test_scope: test_scope.clone(),
        file_scope: state.file_scope(item.file),
        flow_scope,
        own_scope: ScopeKind::Test,
        case: item.case.clone(),
        topology: false,
    };
    let ctx_ud = lua.create_userdata(ctx).expect("create context");

    let file = state.file_path_str(item.file);
    let start = Instant::now();
    let call = item.body.call_async::<()>((ctx_ud, case_arg));

    let result = match item.timeout {
        Some(budget) => match tokio::time::timeout(budget, call).await {
            Ok(r) => r,
            Err(_elapsed) => {
                let assertions = run.borrow().assertions;
                // Teardown still runs after a timeout — and a timed-out test is exactly when a
                // cleanup is most likely to raise, so its errors are reported rather than dropped.
                let errors = teardown_scope(&test_scope).await;
                let mut out = vec![NodeResult {
                    path: item.path.clone(),
                    outcome: Outcome::Failed,
                    duration: start.elapsed(),
                    assertions,
                    message: Some(format!("timed out after {budget:?}")),
                    file: file.clone(),
                    line: item.line,
                    teardown: false,
                    spec: None,
                }];
                out.extend(teardown_results(&item.path, errors, file.as_deref(), item.line));
                return out;
            }
        },
        None => call.await,
    };
    let duration = start.elapsed();

    let (outcome, message, assertions) = {
        let r = run.borrow();
        let (outcome, message) = if r.skip.is_some() {
            (Outcome::Skipped, r.skip.clone())
        } else if let Err(err) = &result {
            (
                Outcome::Failed,
                Some(r.failure.clone().unwrap_or_else(|| err.to_string())),
            )
        } else {
            (Outcome::Passed, None)
        };
        (outcome, message, r.assertions)
    };

    let errors = teardown_scope(&test_scope).await;

    let mut out = vec![NodeResult {
        path: item.path.clone(),
        outcome,
        duration,
        assertions,
        message,
        file: file.clone(),
        line: item.line,
        teardown: false,
        spec: None,
    }];
    out.extend(teardown_results(&item.path, errors, file.as_deref(), item.line));
    out
}

/// A flow is one unit: its steps run serially, in order, on one worker, sharing a `flow`-scope
/// instance. Once a step fails, the remaining steps **cascade-skip** (skip, not fail) with the
/// failing step named. A self-`skip` does not cascade — skip is not failure. The flow scope tears
/// down after the last step (each step's `test` scope having already torn down per-step).
async fn run_flow(lua: &Lua, steps: &[PlanItem], state: &Rc<RunState>) -> Vec<NodeResult> {
    let flow_scope = Rc::new(RefCell::new(ScopeState::default()));
    let mut results = Vec::with_capacity(steps.len());
    let mut cascade: Option<String> = None;

    for step in steps {
        if let Some(reason) = &cascade {
            results.push(NodeResult {
                path: step.path.clone(),
                outcome: Outcome::Skipped,
                duration: Duration::ZERO,
                assertions: 0,
                message: Some(reason.clone()),
                file: state.file_path_str(step.file),
                line: step.line,
                teardown: false,
                spec: None,
            });
            continue;
        }
        let step_results = run_one(lua, step, state, Some(flow_scope.clone())).await;
        if step_results
            .iter()
            .any(|r| !r.teardown && r.outcome == Outcome::Failed)
        {
            let failed = step_name(&step.path);
            cascade = Some(format!("skipped: earlier step {failed:?} failed"));
        }
        results.extend(step_results);
    }

    let errors = teardown_scope(&flow_scope).await;
    let label = steps
        .first()
        .map(|s| flow_label(&s.path))
        .unwrap_or("flow")
        .to_string();
    let file = steps.first().and_then(|s| state.file_path_str(s.file));
    results.extend(teardown_results(&label, errors, file.as_deref(), None));
    results
}

/// A flow's own name — the step path minus its trailing step segment.
fn flow_label(step_path: &str) -> &str {
    match step_path.rfind(" › ") {
        Some(i) => &step_path[..i],
        None => step_path,
    }
}

/// The last path segment — the step's own name, for the cascade-skip message.
fn step_name(path: &str) -> &str {
    path.rsplit(" › ").next().unwrap_or(path)
}

async fn run_unit(lua: &Lua, unit: &PlanUnit, state: &Rc<RunState>) -> Vec<NodeResult> {
    match unit {
        PlanUnit::Test(item) => run_one(lua, item, state, None).await,
        PlanUnit::Flow { steps } => run_flow(lua, steps, state).await,
    }
}

/// The unit-level outcome used for dependency gating: a unit failed if any of its leaf results
/// failed; else passed if any passed; else it was entirely skipped.
fn unit_outcome(results: &[NodeResult]) -> Outcome {
    // Teardown leaves are excluded: `depends_on` gates on whether the unit's *work* passed, and a
    // dependent's premise ("the upstream did its job") still holds when only a cleanup raised.
    // Gating on it would cascade-skip a whole subgraph over a leaked container.
    //
    // An open spec (`Outcome::Spec`) gates like a failure: the upstream did NOT do its job — its
    // implementation doesn't exist yet — so a dependent's premise cannot hold. Only the *report*
    // treats an open spec gently; the DAG does not.
    let work = || results.iter().filter(|r| !r.teardown);
    if work().any(|r| matches!(r.outcome, Outcome::Failed | Outcome::Spec)) {
        Outcome::Failed
    } else if work().any(|r| r.outcome == Outcome::Passed) {
        Outcome::Passed
    } else {
        Outcome::Skipped
    }
}

/// Build skipped results for a unit that never ran (a dependency did not pass) — one per reported
/// path (a flow reports one skip per step), so the report stays consistent with a unit that ran.
fn skip_leaf(unit: &PlanUnit, reason: &str, state: &RunState) -> Vec<NodeResult> {
    unit.items()
        .into_iter()
        .map(|item| NodeResult {
            path: item.path.clone(),
            outcome: Outcome::Skipped,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some(reason.to_string()),
            file: state.file_path_str(item.file),
            line: item.line,
            teardown: false,
            spec: None,
        })
        .collect()
}

fn emit_finished(reporter: &mut dyn Reporter, summary: &mut Summary, results: &[NodeResult]) {
    for result in results {
        summary.tally(result.outcome);
        reporter.event(&Event::NodeFinished {
            path: &result.path,
            outcome: result.outcome,
            duration: result.duration,
            assertions: result.assertions,
            message: result.message.as_deref(),
            file: result.file.as_deref(),
            line: result.line,
            spec_reason: result.spec.as_deref(),
        });
    }
}

/// A readers-writer accounting table over resource tokens. Per token it tracks how many shared
/// (reader) and exclusive (writer) holds are live. A reader may acquire when there is no writer; a
/// writer may acquire only when there is neither reader nor writer. Acquisition is all-or-nothing
/// per leaf (checked before any hold is taken), so no leaf ever holds-and-waits — hence no deadlock.
#[derive(Default)]
struct ResourceTable {
    holders: HashMap<String, (u32, u32)>, // token -> (shared, exclusive)
}

impl ResourceTable {
    fn can_acquire(&self, reqs: &[ResourceReq]) -> bool {
        reqs.iter().all(|r| {
            let (shared, exclusive) = self.holders.get(&r.token).copied().unwrap_or((0, 0));
            if r.shared {
                exclusive == 0
            } else {
                shared == 0 && exclusive == 0
            }
        })
    }

    fn acquire(&mut self, reqs: &[ResourceReq]) {
        for r in reqs {
            let entry = self.holders.entry(r.token.clone()).or_insert((0, 0));
            if r.shared {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
    }

    fn release(&mut self, reqs: &[ResourceReq]) {
        for r in reqs {
            if let Some(entry) = self.holders.get_mut(&r.token) {
                if r.shared {
                    entry.0 = entry.0.saturating_sub(1);
                } else {
                    entry.1 = entry.1.saturating_sub(1);
                }
            }
        }
    }
}

/// Dependency- and resource-aware scheduler. A leaf runs once all its dependency leaves have
/// **passed** *and* its declared resources can be acquired (readers-writer); if any dependency
/// failed or was skipped, the leaf cascade-skips (transitively). Independent, resource-compatible
/// leaves run concurrently up to `config.concurrency`; with the default of 1 this is
/// definition-order sequential and resource declarations are inert.
async fn run_plan(
    lua: &Lua,
    plan: &Plan,
    state: &Rc<RunState>,
    config: &RunConfig,
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
) {
    let leaves = &plan.leaves;
    let n = leaves.len();
    let concurrency = config.concurrency.max(1);
    let mut outcome: Vec<Option<Outcome>> = vec![None; n];
    let mut started = vec![false; n];
    let mut resources = ResourceTable::default();
    let mut in_flight = futures::stream::FuturesUnordered::new();

    loop {
        // Skip to a fixpoint: a leaf is skipped without running when it has an unmet `requires`
        // (a precondition skip, independent of deps) or all its deps are resolved but not all passed
        // (a cascade skip). Looping catches transitive skips in one pass.
        let mut changed = true;
        while changed {
            changed = false;
            for i in 0..n {
                if started[i] || outcome[i].is_some() {
                    continue;
                }
                let reason = if let Some(reason) = &leaves[i].precondition_skip {
                    Some(reason.clone())
                } else if !leaves[i].deps.iter().all(|&d| outcome[d].is_some()) {
                    None // deps not all resolved yet — decide later
                } else {
                    leaves[i]
                        .deps
                        .iter()
                        .find(|&&d| outcome[d] != Some(Outcome::Passed))
                        .map(|&blocker| {
                            format!(
                                "skipped: dependency {:?} did not pass",
                                unit_name(&leaves[blocker])
                            )
                        })
                };
                if let Some(reason) = reason {
                    let results = skip_leaf(&leaves[i].unit, &reason, state);
                    for path in leaves[i].unit.leaf_paths() {
                        reporter.event(&Event::NodeStarted { path });
                    }
                    emit_finished(reporter, summary, &results);
                    outcome[i] = Some(Outcome::Skipped);
                    started[i] = true;
                    changed = true;
                }
            }
        }

        // Launch runnable leaves — all deps passed and resources acquirable — up to the concurrency
        // limit. A resource-blocked leaf is left for a later round (a completion frees its holds).
        for i in 0..n {
            if in_flight.len() >= concurrency {
                break;
            }
            if started[i] || outcome[i].is_some() {
                continue;
            }
            if !leaves[i]
                .deps
                .iter()
                .all(|&d| outcome[d] == Some(Outcome::Passed))
            {
                continue;
            }
            if !resources.can_acquire(&leaves[i].reqs) {
                continue;
            }
            resources.acquire(&leaves[i].reqs);
            started[i] = true;
            for path in leaves[i].unit.leaf_paths() {
                reporter.event(&Event::NodeStarted { path });
            }
            in_flight.push(async move { (i, run_unit(lua, &leaves[i].unit, state).await) });
        }

        if in_flight.is_empty() {
            break; // nothing running and nothing became ready — all leaves resolved
        }

        let (i, mut results) = in_flight.next().await.expect("in_flight is non-empty");
        resources.release(&leaves[i].reqs);
        // A spec-flagged leaf's results are inverted BEFORE gating and reporting: red → open spec
        // (or a real failure under --strict-specs), green → "graduate it". Gating sees the
        // post-inversion truth, so a dependent of an open spec still cascade-skips.
        if let Some(reason) = &leaves[i].spec {
            apply_spec_inversion(&mut results, reason, config.strict_specs);
        }
        outcome[i] = Some(unit_outcome(&results));
        emit_finished(reporter, summary, &results);
    }
}

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

/// Build plan → run → tear down (every file scope, then the suite). Shared by the single-file and
/// multi-file loaders once the collector is populated.
fn execute_collected(
    lua: &Lua,
    col: &SharedCollector,
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    let (plan, deselected, state) = {
        let col = col.borrow();
        let plan = build_plan(&col, &config.capabilities)?;
        let (plan, deselected) = apply_selection(plan, &config.selection);
        let (plan, spec_deselected) = apply_specs_filter(plan, config.specs_only);
        let deselected = deselected + spec_deselected;
        let state = Rc::new(RunState {
            defs: col.fixtures.clone(),
            suite: Rc::new(RefCell::new(ScopeState::default())),
            files: RefCell::new(HashMap::new()),
            file_paths: col.file_paths.clone(),
            update_snapshots: config.update_snapshots,
            snapshot_registry: config.snapshot_registry.clone(),
        });
        (plan, deselected, state)
    };

    let rt = new_runtime()?;
    let mut summary = Summary {
        deselected,
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

// ---------------------------------------------------------------------------------------------
// `prova eval` — a one-shot snippet in the full environment
// ---------------------------------------------------------------------------------------------

/// Run a one-shot Lua snippet in the full prova environment — built-in modules (`shell`, `fs`,
/// `docker`, …), host modules, and manifest-declared plugins via `require` — with a global `ctx`
/// backed by a real transient scope, then convert the returned value to JSON for the caller.
///
/// The snippet may be an expression or statements: it is first compiled as `return (<code>)` (so
/// `1 + 1` works bare), falling back to the raw source (multi-statement snippets write their own
/// `return`). It runs via the async call path inside the per-run Tokio runtime, so async
/// functions (a plugin's `container()`, `shell.run`, `prova.sleep`) work exactly as they do in
/// tests. Everything `ctx:defer`/`ctx:manage` registered is torn down — success *or* error —
/// inside that same runtime before this returns, so provisioned resources never outlive the eval.
/// Load the project's optional `prova.lua` companion — the project-level home for
/// `runtime.capability(name, fn)` (and the `runtime.*` config DSL generally).
///
/// **Why a companion and not `suite.lua`** (docs/design/test-topology.md): a capability is a
/// project-wide vocabulary, so registering it per-suite would leave it invisible to sibling suites
/// and to `must_run` — and `must_run` is a PRECONDITION checked before any suite loads, so a
/// suite-registered capability would not exist yet at the moment it is needed. Loading with the
/// manifest is what makes `must_run = ["gpu"]` possible at all.
///
/// Each predicate is evaluated HERE, at load, and its verdict stored — see [`REGISTERED_CAPS`].
/// The predicate may answer:
///   - `true`            → available, no version
///   - a version string  → available, and comparable (`requires = { "gpu >= 2.0" }`)
///   - `false` / `nil`   → unavailable
///
/// A companion that fails to load is an **error**, never a warning: every capability it meant to
/// register would silently go missing, so every gated test would skip and the run would be green —
/// the vacuous green, one level further out than the suite.
pub fn load_project_config(
    path: &std::path::Path,
    config: &RunConfig,
) -> Result<Capabilities, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let (lua, _col) =
        build_lua("config".to_string(), config).map_err(|e| format!("{}: {e}", path.display()))?;

    // The companion's registrations accumulate HERE — a per-load value, returned to the caller, not
    // a process global. Two projects loaded in one process (the warm MCP) each get their own.
    let caps = std::rc::Rc::new(std::cell::RefCell::new(Capabilities::default()));
    let caps_w = caps.clone();

    let registrar = lua
        .create_function(move |_, (name, f): (String, mlua::Function)| {
            if is_builtin_capability(&name) {
                return Err(mlua::Error::RuntimeError(format!(
                    "runtime.capability({name:?}): {name:?} is a built-in capability and cannot be \
                     redefined — `requires = {{ {name:?} }}` must mean the same thing in every project"
                )));
            }
            // The predicate runs NOW, at load; only its answer survives (see `Capabilities`).
            let verdict: Value = f.call(())?;
            match verdict {
                // Unavailable → not registered, so it reads as absent everywhere.
                Value::Nil | Value::Boolean(false) => {}
                // Available, no version.
                Value::Boolean(true) => caps_w.borrow_mut().register(&name, None),
                // Available, and it reported a version to compare against.
                Value::String(s) => {
                    let raw = s.to_str()?.to_string();
                    let v = parse_first_version(&raw).ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "runtime.capability({name:?}): returned {raw:?}, which is not a version \
                             (expected true/false, or a version string like \"2.4.0\")"
                        ))
                    })?;
                    caps_w.borrow_mut().register(&name, Some(v));
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "runtime.capability({name:?}): the predicate returned {}, expected a boolean \
                         or a version string",
                        other.type_name()
                    )))
                }
            }
            Ok(())
        })
        .map_err(|e| format!("{}: {e}", path.display()))?;

    // `runtime` — the Lua-shaped configuration DSL for the whole runtime, available ONLY here in the
    // companion. It is deliberately NOT on `prova` (the test-authoring surface): configuring the
    // environment tests run *in* is a different job from writing tests, and keeping it a separate
    // global is what makes "you can't call this in a test" a self-evident error rather than a
    // baffling nil on `prova`.
    let runtime = lua
        .create_table()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    runtime
        .set("capability", registrar)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    lua.globals()
        .set("runtime", runtime)
        .map_err(|e| format!("{}: {e}", path.display()))?;

    lua.load(&src)
        .set_name(file_chunk_name(path))
        .exec()
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let out = caps.borrow().clone();
    Ok(out)
}

pub fn eval_snippet(code: &str, config: &RunConfig) -> mlua::Result<serde_json::Value> {
    let (lua, col) = build_lua("eval".to_string(), config)?;

    // One transient scope for the whole eval, over a state that knows the snippet's fixtures.
    let state = Rc::new(RunState {
        defs: col.borrow().fixtures.clone(),
        suite: Rc::new(RefCell::new(ScopeState::default())),
        files: RefCell::new(HashMap::new()),
        file_paths: Vec::new(),
        update_snapshots: false,
        snapshot_registry: None,
    });

    let rt = new_runtime()?;
    eval_with_state(&lua, &rt, code, &state)
}

/// The shared eval executor: compile `code`, expose a transient `ctx` over `state`'s File scope,
/// run it inside `rt`, tear the transient scope down (success OR error), and JSON-ify the value.
/// Used by the one-shot `eval_snippet` (fresh Lua/runtime) and by `HeldTopology::eval_warm` (the
/// holder's Lua/runtime, with the held instance pre-seeded into `state`).
fn eval_with_state(
    lua: &Lua,
    rt: &tokio::runtime::Runtime,
    code: &str,
    state: &Rc<RunState>,
) -> mlua::Result<serde_json::Value> {
    // Prefer the expression wrapping; fall back to raw statements. The newline before `)` keeps a
    // trailing `-- comment` in the snippet from swallowing the wrapper's close paren.
    let chunk = match lua
        .load(format!("return ({code}\n)"))
        .set_name("eval")
        .into_function()
    {
        Ok(f) => f,
        Err(_) => lua.load(code).set_name("eval").into_function()?,
    };

    // A File-scope context, exactly like `prova up`'s provisioner (no test scope exists here; the
    // File scope stands in for `defer`/`manage`).
    let file0 = state.file_scope(0);
    let ctx = Ctx {
        run: Rc::new(RefCell::new(TestRun::default())),
        state: state.clone(),
        test_scope: file0.clone(),
        file_scope: file0,
        flow_scope: None,
        own_scope: ScopeKind::File,
        case: None,
        topology: false,
    };
    lua.globals().set("ctx", lua.create_userdata(ctx)?)?;

    let value = block_on_local(rt, async {
        let outcome = chunk.call_async::<Value>(()).await;
        // Tear the transient scope down inside the same runtime, success OR error (mirroring
        // execute_collected), so whatever the snippet provisioned is reaped before we return.
        teardown_all_and_warn(state).await;
        outcome
    })?;
    Ok(eval_value_to_json(lua, &value, 0))
}

/// Convert an eval result to JSON, defensively: primitives map directly, tables become arrays
/// (pure sequences) or objects, and anything without a JSON form — userdata, functions, threads,
/// non-finite numbers — degrades to its `tostring()` string. The eval already succeeded; reporting
/// its value must never raise or panic.
fn eval_value_to_json(lua: &Lua, v: &Value, depth: usize) -> serde_json::Value {
    use serde_json::Value as J;
    if depth > 64 {
        return J::String("<table nested too deeply (or cyclic)>".into());
    }
    match v {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => J::Number((*i).into()),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(J::Number)
            .unwrap_or_else(|| J::String(n.to_string())), // NaN/±inf have no JSON number form
        Value::String(s) => J::String(s.to_string_lossy().to_string()),
        Value::Table(t) => {
            let len = t.raw_len();
            let pairs: Vec<(Value, Value)> = t
                .clone()
                .pairs::<Value, Value>()
                .filter_map(|p| p.ok())
                .collect();
            // A pure sequence (keys are exactly 1..#t) is a JSON array; anything else an object.
            if len > 0 && pairs.len() == len {
                J::Array(
                    (1..=len)
                        .map(|i| {
                            let item = t.raw_get::<Value>(i).unwrap_or(Value::Nil);
                            eval_value_to_json(lua, &item, depth + 1)
                        })
                        .collect(),
                )
            } else {
                let mut map = serde_json::Map::new();
                for (k, val) in pairs {
                    let key = match &k {
                        Value::String(s) => s.to_string_lossy().to_string(),
                        other => eval_tostring(lua, other),
                    };
                    map.insert(key, eval_value_to_json(lua, &val, depth + 1));
                }
                J::Object(map)
            }
        }
        other => J::String(eval_tostring(lua, other)),
    }
}

/// `tostring(v)` through Lua (honors `__tostring`), with a typename fallback if even that raises.
fn eval_tostring(lua: &Lua, v: &Value) -> String {
    lua.globals()
        .get::<Function>("tostring")
        .and_then(|f| f.call::<String>(v.clone()))
        .unwrap_or_else(|_| format!("<{}>", v.type_name()))
}

// ---------------------------------------------------------------------------------------------
// `prova up` — stand up a named topology and hold it (the same definition tests use)
// ---------------------------------------------------------------------------------------------

/// A resource endpoint reported by `prova up` — a topology field name and its connect URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub name: String,
    pub url: String,
}

/// Stand up the topology named `name` from `files`, report its endpoints via `on_ready`, and hold it
/// running until a shutdown signal (SIGINT / SIGTERM), then tear it down. The files are loaded into
/// one Lua state (so a topology may live in a setup file or any test file). `on_ready` is called once,
/// after provisioning succeeds, with the resources' endpoints — the caller prints them and records any
/// run state. Returns after teardown completes (or immediately with an error if provisioning fails,
/// having still torn down any partial resources).
pub fn up(
    files: &[PathBuf],
    name: &str,
    config: &RunConfig,
    on_ready: impl FnOnce(&[Endpoint]),
) -> mlua::Result<()> {
    let (lua, _col, state, id) = load_topology(files, name, config)?;

    let rt = new_runtime()?;
    block_on_local(&rt, async {
        let result = provision_and_hold(&lua, &state, id, name, on_ready).await;
        // Always tear down whatever got provisioned — a clean signal, or a mid-provision failure.
        teardown_all_and_warn(&state).await;
        result
    })
}

/// `prova watch <name>` — the inhabited dev loop. Provision the topology, report its endpoints, and
/// hold; when any of `files` changes on disk, tear down and re-provision from the *fresh* definition
/// (a new Lua state, so edits take effect), reporting the new endpoints. Repeats until a shutdown
/// signal, then tears down and returns. `on_ready(endpoints, reapply)` is called after each successful
/// (re)provision (`reapply` is false the first time). A definition that fails to provision (e.g. a bad
/// edit) is reported via `on_error` and does *not* exit the loop — the watcher waits for the next
/// change so the fix is picked up. Use `--fixed` for stable endpoints across re-applies.
pub fn watch(
    files: &[PathBuf],
    name: &str,
    config: &RunConfig,
    mut on_ready: impl FnMut(&[Endpoint], bool),
    mut on_error: impl FnMut(&mlua::Error),
) -> mlua::Result<()> {
    let rt = new_runtime()?;
    block_on_local(&rt, async {
        let mut reapply = false;
        loop {
            // Build a fresh state each pass so a changed definition is actually re-read.
            match load_topology(files, name, config) {
                Ok((lua, _col, state, id)) => {
                    let held = async {
                        let (_value, endpoints) = provision(&lua, &state, id, name).await?;
                        on_ready(&endpoints, reapply);
                        Ok::<bool, mlua::Error>(wait_for_change_or_shutdown(files).await)
                    }
                    .await;
                    teardown_all_and_warn(&state).await;
                    match held {
                        // A file changed → loop and re-provision. Shutdown → done.
                        Ok(true) => {}
                        Ok(false) => return Ok(()),
                        // Provisioning itself failed: report, then wait for the next edit or a signal.
                        Err(e) => {
                            on_error(&e);
                            if !wait_for_change_or_shutdown(files).await {
                                return Ok(());
                            }
                        }
                    }
                }
                // The files don't even load / no such topology — a hard error worth surfacing to exit.
                Err(e) => return Err(e),
            }
            reapply = true;
        }
    })
}

/// Load `files` into a fresh Lua state and resolve the named topology's fixture id, returning the
/// state pieces `provision` needs. Shared by `up`, `watch`, and `hold_topology` (which keeps the
/// collector so warm runs can reset and re-collect in the same state).
/// A manifest topology (`[topologies]`), desugared to `prova.topology(alias, require(plugin).factory)`.
#[derive(Debug, Clone)]
pub struct TopologyRegistration {
    pub alias: String,
    pub plugin: String,
    pub factory: String,
    /// A pre-serialized Lua table literal passed to the factory as a second argument, or `None` to
    /// register the factory bare. The CLI (which owns the manifest's `toml`) produces the literal, so
    /// only well-formed literals reach here; a malformed one surfaces as a Lua parse error, never a
    /// silent hole.
    pub options: Option<String>,
}

/// Register the manifest topologies into an already-built `lua`: exec one
/// `prova.topology("<alias>", (require("<plugin>")).<factory>)` per registration. Must run AFTER the
/// definition files (so a manifest topology can override or add to what a suite declared) and after
/// the plugin searcher is installed (so `require` resolves).
///
/// The three fields are validated against a conservative shape before being spliced into Lua source,
/// so a manifest can never inject code — an out-of-shape value is a clear error, not a silent hole.
fn exec_topology_registrations(lua: &Lua, config: &RunConfig) -> mlua::Result<()> {
    let is_ident_path = |s: &str| {
        !s.is_empty()
            && s.split('.').all(|seg| {
                let mut c = seg.chars();
                c.next()
                    .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
                    && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            })
    };
    let is_alias = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    for r in &config.topology_registrations {
        if !is_alias(&r.alias) || !is_ident_path(&r.plugin) || !is_ident_path(&r.factory) {
            return Err(mlua::Error::RuntimeError(format!(
                "invalid [topologies] entry {:?}: name must be [A-Za-z0-9_-]+, and plugin/factory \
                 dotted identifier paths (got plugin={:?}, factory={:?})",
                r.alias, r.plugin, r.factory
            )));
        }
        // Bare: register the factory itself (called with `(ctx)`). With options: wrap so the factory
        // receives them as a second argument, `factory(ctx, <options>)`. The options literal is
        // produced by the CLI's serializer, whose output is a self-contained Lua value expression.
        let code = match &r.options {
            None => format!(
                "prova.topology(\"{}\", (require(\"{}\")).{})",
                r.alias, r.plugin, r.factory
            ),
            Some(opts) => format!(
                "prova.topology(\"{}\", function(ctx) return (require(\"{}\")).{}(ctx, {}) end)",
                r.alias, r.plugin, r.factory, opts
            ),
        };
        lua.load(&code)
            .set_name(format!("@[topologies].{}", r.alias))
            .exec()
            .map_err(|e| {
                mlua::Error::RuntimeError(format!(
                    "topology {:?} (require(\"{}\").{}): {e}",
                    r.alias, r.plugin, r.factory
                ))
            })?;
    }
    Ok(())
}

/// Enumerate the topology names available — every `prova.topology(name, fn)` the `files` declare,
/// plus every `[topologies]` registration — sorted. Only *registers* them (execs the files); it never
/// invokes a factory, so it needs no docker. The discovery half of `up` (`prova up` with no name).
pub fn list_topologies(files: &[PathBuf], config: &RunConfig) -> mlua::Result<Vec<String>> {
    let (lua, col) = build_lua("up".to_string(), config)?;
    for file in files {
        let code = std::fs::read_to_string(file).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", file.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(file)).exec()?;
    }
    exec_topology_registrations(&lua, config)?;
    let names: Vec<String> = col.borrow().topologies.keys().cloned().collect();
    Ok(names)
}

fn load_topology(
    files: &[PathBuf],
    name: &str,
    config: &RunConfig,
) -> mlua::Result<(Lua, SharedCollector, Rc<RunState>, usize)> {
    let (lua, col) = build_lua("up".to_string(), config)?;
    for file in files {
        let code = std::fs::read_to_string(file).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", file.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(file)).exec()?;
    }
    exec_topology_registrations(&lua, config)?;

    let id = {
        let c = col.borrow();
        match c.topologies.get(name) {
            Some(id) => *id,
            None => {
                let hint = if c.topologies.is_empty() {
                    "no topologies are defined (declare one with prova.topology(name, fn))"
                        .to_string()
                } else {
                    let avail: Vec<&str> = c.topologies.keys().map(String::as_str).collect();
                    format!("available: {}", avail.join(", "))
                };
                return Err(mlua::Error::RuntimeError(format!(
                    "no topology named {name:?} ({hint})"
                )));
            }
        }
    };

    let state = Rc::new(RunState {
        defs: col.borrow().fixtures.clone(),
        suite: Rc::new(RefCell::new(ScopeState::default())),
        files: RefCell::new(HashMap::new()),
        file_paths: col.borrow().file_paths.clone(),
        update_snapshots: false, // snapshots are a test-mode concern, not for inhabited topologies
        snapshot_registry: None,
    });
    Ok((lua, col, state, id))
}

/// Instantiate the topology under a held `Scope.File`, report its endpoints, and block until a
/// shutdown signal. Separated so `up` can run teardown unconditionally afterward — even if the factory
/// raises mid-provision, the File scope already holds teardowns for whatever came up.
async fn provision_and_hold(
    lua: &Lua,
    state: &Rc<RunState>,
    id: usize,
    topo_name: &str,
    on_ready: impl FnOnce(&[Endpoint]),
) -> mlua::Result<()> {
    let (_value, endpoints) = provision(lua, state, id, topo_name).await?;
    on_ready(&endpoints);
    wait_for_shutdown().await;
    Ok(())
}

/// Instantiate the topology under a held `Scope.File` and return its live value plus its endpoints.
/// The provisioned resources stay alive via the File scope's teardowns (held in `state`) until the
/// caller reaps them; separated from the wait/hold so `up` (hold until signal), `watch` (hold until
/// change), and `hold_topology` (hold across MCP tool calls) all reuse it.
async fn provision(
    lua: &Lua,
    state: &Rc<RunState>,
    id: usize,
    topo_name: &str,
) -> mlua::Result<(Value, Vec<Endpoint>)> {
    let file0 = state.file_scope(0);
    let ctx = Ctx {
        run: Rc::new(RefCell::new(TestRun::default())),
        state: state.clone(),
        test_scope: file0.clone(), // no test scope in `up`; the File scope stands in for `manage`
        file_scope: file0,
        flow_scope: None,
        own_scope: ScopeKind::File,
        case: None,
        topology: false,
    };
    let handle = lua.create_userdata(FixtureHandle { id })?;
    let value = resolve_use(lua, &ctx, Value::UserData(handle)).await?;
    let endpoints = extract_endpoints(&value, topo_name);
    Ok((value, endpoints))
}

/// Walk a topology's returned value for connect strings. Each field whose value is a table with a
/// string `url` becomes an endpoint (`db → postgres://…`); a top-level `url` (a single-resource
/// topology) is reported under the topology's own name.
fn extract_endpoints(value: &Value, topo_name: &str) -> Vec<Endpoint> {
    let mut out = Vec::new();
    if let Value::Table(t) = value {
        if let Ok(Value::String(u)) = t.get::<Value>("url") {
            out.push(Endpoint {
                name: topo_name.to_string(),
                url: u.to_string_lossy().to_string(),
            });
        }
        for pair in t.pairs::<Value, Value>() {
            let Ok((Value::String(key), Value::Table(rt))) = pair else {
                continue;
            };
            if let Ok(Value::String(u)) = rt.get::<Value>("url") {
                out.push(Endpoint {
                    name: key.to_string_lossy().to_string(),
                    url: u.to_string_lossy().to_string(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ---------------------------------------------------------------------------------------------
// Warm topology holding (`prova mcp`: up / run{topology} / eval{topology} / down)
// ---------------------------------------------------------------------------------------------

/// A named topology provisioned and **held inside this process** — the warm phase of MCP mode
/// (docs/design/mcp-mode.md "Warm re-run"). Owns the Lua state the topology lives in, the run
/// state whose held File scope carries the topology's teardowns, and the Tokio runtime that
/// provisioned it (held resources — clients, pools, containers — may be bound to that runtime, so
/// every warm call runs under it).
///
/// **Same-Lua warmth**: `run_warm` re-collects the project's files into this same Lua state and
/// injects the held instance into the fresh run's scope caches keyed by topology *name*, so
/// `t:use(<topology>)` resolves the identical live Lua values instead of provisioning.
///
/// **Ownership**: warm runs and evals tear down only their own transient scopes (the held value is
/// injected as a cached *value*, never as a teardown), so the holder — `teardown()`, driven by the
/// MCP `down` tool or server shutdown — is the one true reaper.
///
/// `Lua` is `!Send`, so a `HeldTopology` must be created, used, and dropped on one thread (the MCP
/// server confines each one to a dedicated holder thread driven by a command channel).
pub struct HeldTopology {
    name: String,
    lua: Lua,
    /// The collector captured by this state's `prova.*` closures — reset and re-populated per warm
    /// run (fresh collection, held values).
    col: SharedCollector,
    /// The holder's run state: its File scope owns the provisioning teardowns.
    state: Rc<RunState>,
    /// The held instance — the topology factory's returned value, alive for the holder's lifetime.
    value: Value,
    endpoints: Vec<Endpoint>,
    rt: tokio::runtime::Runtime,
    config: RunConfig,
}

/// Stand up the topology named `name` from `files` and hold it in-process: the factory runs exactly
/// once, its teardowns are parked on the returned holder, and the held value is also published as a
/// Lua **global named after the topology** (so `eval_warm` snippets can address it directly, e.g.
/// `return orders.db.url`). A mid-provision failure still reaps whatever came up before erroring.
pub fn hold_topology(
    files: &[PathBuf],
    name: &str,
    config: &RunConfig,
) -> mlua::Result<HeldTopology> {
    let (lua, col, state, id) = load_topology(files, name, config)?;
    let rt = new_runtime()?;
    let provisioned = block_on_local(&rt, async {
        match provision(&lua, &state, id, name).await {
            Ok(v) => Ok(v),
            Err(e) => {
                // Partial provisioning already parked teardowns for whatever came up — reap them.
                teardown_all_and_warn(&state).await;
                Err(e)
            }
        }
    });
    let (value, endpoints) = provisioned?;
    lua.globals().set(name, value.clone())?;
    Ok(HeldTopology {
        name: name.to_string(),
        lua,
        col,
        state,
        value,
        endpoints,
        rt,
        config: config.clone(),
    })
}

impl HeldTopology {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The endpoints reported at provisioning time (`name → url`), for `up` results and `status`.
    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    /// A **warm run**: re-read `files` from disk (edits since `up` take effect), collect them into
    /// this holder's Lua state (collector reset, same VM), and run the plan with the held topology
    /// instance injected — `t:use(<topology>)` resolves the very same live Lua values the holder
    /// provisioned, so held state accumulates across runs and the factory never re-runs.
    pub fn run_warm(
        &self,
        files: &[PathBuf],
        selection: &Selection,
        reporter: &mut dyn Reporter,
    ) -> mlua::Result<Summary> {
        // Fresh collection in the held state: reset the collector the `prova.*` globals write to,
        // then load exactly as a cold suite would (one file at index 0; several under per-file
        // groups), so node paths and selection match their cold-run spelling.
        if files.len() == 1 {
            let stem = files[0]
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("tests")
                .to_string();
            *self.col.borrow_mut() = Collector::new(stem);
            self.col.borrow_mut().set_file_path(0, &files[0]);
            let code = std::fs::read_to_string(&files[0]).map_err(|e| {
                mlua::Error::RuntimeError(format!("cannot read {}: {e}", files[0].display()))
            })?;
            self.lua
                .load(&code)
                .set_name(file_chunk_name(&files[0]))
                .exec()?;
        } else {
            *self.col.borrow_mut() = Collector::new(self.name.clone());
            load_member_files(&self.lua, &self.col, files)?;
        }

        let (plan, deselected, state) = {
            let col = self.col.borrow();
            let plan = build_plan(&col, &self.config.capabilities)?;
            let (plan, deselected) = apply_selection(plan, selection);
            let (plan, spec_deselected) = apply_specs_filter(plan, self.config.specs_only);
            let deselected = deselected + spec_deselected;

            // A fresh run state — the run's own scopes, so its teardown reaps only what it built.
            let state = Rc::new(RunState {
                defs: col.fixtures.clone(),
                suite: Rc::new(RefCell::new(ScopeState::default())),
                files: RefCell::new(HashMap::new()),
                file_paths: col.file_paths.clone(),
                update_snapshots: self.config.update_snapshots,
                snapshot_registry: self.config.snapshot_registry.clone(),
            });

            // Held-instance injection, keyed by topology NAME (topologies are name-addressable by
            // design): the fresh collection re-declared the topology under a new fixture id — seed
            // that id's value into the suite scope and every file scope, so `t:use` cache-hits from
            // whichever scope the (re-read) declaration targets, instead of running the factory.
            // The value goes in *without* a teardown entry: the holder remains the only reaper.
            let id = *col.topologies.get(&self.name).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "held topology {:?} is no longer defined by the project's files",
                    self.name
                ))
            })?;
            state
                .suite
                .borrow_mut()
                .cache
                .insert(id, self.value.clone());
            for idx in 0..=files.len() {
                state
                    .file_scope(idx)
                    .borrow_mut()
                    .cache
                    .insert(id, self.value.clone());
            }
            (plan, deselected, state)
        };

        let mut config = self.config.clone();
        config.selection = selection.clone();

        reporter.event(&Event::RunStarted);
        let mut summary = Summary {
            deselected,
            ..Summary::default()
        };
        // The holder's runtime, not a fresh one: held resources may be bound to it.
        block_on_local(&self.rt, async {
            let started = Instant::now();
            run_plan(&self.lua, &plan, &state, &config, reporter, &mut summary).await;
            // Tear down the run's own scopes only. The injected instance is a cached value with no
            // teardown registered here; its teardowns stay parked on the holder's state.
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
        reporter.event(&Event::RunFinished { summary: &summary });
        Ok(summary)
    }

    /// A **warm eval**: run a one-shot snippet in the holder's Lua state, where the held value is a
    /// global named after the topology (set at hold time) and `ctx:use(<name>)` resolves the held
    /// instance. The snippet's own `defer`/`manage` teardowns run afterwards; the held instance's
    /// never do.
    pub fn eval_warm(&self, code: &str) -> mlua::Result<serde_json::Value> {
        // A transient state seeded with the held instance, so `ctx:use` is warm too. The current
        // collector still describes the most recent collection in this VM, so its fixture ids line
        // up with any handles a snippet might reference.
        let state = Rc::new(RunState {
            defs: self.col.borrow().fixtures.clone(),
            suite: Rc::new(RefCell::new(ScopeState::default())),
            files: RefCell::new(HashMap::new()),
            file_paths: Vec::new(),
            update_snapshots: false,
            snapshot_registry: None,
        });
        if let Some(&id) = self.col.borrow().topologies.get(&self.name) {
            state
                .suite
                .borrow_mut()
                .cache
                .insert(id, self.value.clone());
            state
                .file_scope(0)
                .borrow_mut()
                .cache
                .insert(id, self.value.clone());
        }
        eval_with_state(&self.lua, &self.rt, code, &state)
    }

    /// The one true teardown: run everything the provisioning parked on the holder's scopes
    /// (`ctx:defer`/`ctx:manage`, LIFO), consuming the holder. Driven by the MCP `down` tool or by
    /// server shutdown — never by a warm run.
    ///
    /// There is no reporter here — nobody is running tests — so failures go to stderr. They must go
    /// *somewhere*: this is the path that stops a held topology's containers, so a teardown that
    /// raised is a container still running on the operator's machine after `down` said it was done.
    /// Silence there is the worst possible answer.
    pub fn teardown(self) {
        block_on_local(&self.rt, async {
            teardown_all_and_warn(&self.state).await;
        });
    }
}

/// Block until the user (Ctrl-C / SIGINT) or a supervisor (`prova down`, via SIGTERM) asks to shut
/// down. Handling SIGTERM here is what lets the detached `start`/`down` layer tear an environment down.
async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Block until either a watched file changes on disk (returns `true` — re-apply) or a shutdown signal
/// arrives (returns `false` — stop). Dependency-free: polls the files' modification times against a
/// snapshot taken at entry. A short settle after a detected change lets an editor's multi-write save
/// finish before we re-provision, so one save triggers one re-apply.
async fn wait_for_change_or_shutdown(files: &[PathBuf]) -> bool {
    let baseline = snapshot_mtimes(files);
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(400));
    ticker.tick().await; // the first tick completes immediately; skip it
    loop {
        tokio::select! {
            _ = wait_for_shutdown() => return false,
            _ = ticker.tick() => {
                if snapshot_mtimes(files) != baseline {
                    // Let a burst of writes settle, then confirm before re-provisioning.
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    return true;
                }
            }
        }
    }
}

/// Each file's last-modified time (`None` if it can't be stat'd — e.g. mid-rename), positional so a
/// simple `!=` against a baseline detects any change, appearance, or disappearance.
fn snapshot_mtimes(files: &[PathBuf]) -> Vec<Option<std::time::SystemTime>> {
    files
        .iter()
        .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .collect()
}

/// An empty labeling `Group` node (a file-group). `file`/parent are set by `add`.
fn group_node(name: String) -> Node {
    Node {
        name,
        kind: NodeKind::Group,
        params: Params::default(),
        opts: UnitOpts::default(),
        children: vec![],
        body: None,
        case: None,
        file: 0,
        line: None,
    }
}

/// Tear down every per-file `Scope.File` instance (a suite may have several).
/// Tear down every file scope, returning any failures as reported leaves.
///
/// Keyed by file *index* rather than a path because that is the identity the scope map carries;
/// where a real path is known it names the node, so a report says which file leaked.
async fn teardown_file_scopes(state: &RunState) -> Vec<NodeResult> {
    let scopes: Vec<(usize, Rc<RefCell<ScopeState>>)> = state
        .files
        .borrow()
        .iter()
        .map(|(i, s)| (*i, s.clone()))
        .collect();
    let mut out = Vec::new();
    for (idx, scope) in scopes {
        let errors = teardown_scope(&scope).await;
        if errors.is_empty() {
            continue;
        }
        let file = state.file_path_str(idx);
        let label = file.clone().unwrap_or_else(|| format!("file {idx}"));
        out.extend(teardown_results(&label, errors, file.as_deref(), None));
    }
    out
}

/// Discovery: collect the test tree without executing tests (basis for a GUI/IDE model view).
pub fn discover_path(path: &Path) -> mlua::Result<Vec<String>> {
    discover_path_with(path, &RunConfig::new(1))
}

/// Discovery with plugin modules installed. Collection runs the file's top level, so any plugin
/// global used there (e.g. `archetect.verify` registering tests) must exist during discovery too —
/// pass the same `RunConfig` you would run with.
pub fn discover_path_with(path: &Path, config: &RunConfig) -> mlua::Result<Vec<String>> {
    let (_lua, col) = read_and_collect(path, config)?;
    let col = col.borrow();
    list_plan(&col, config)
}

/// Discover node paths for a whole **suite** — the setup (`suite.lua`) loads first, exactly as in
/// `run_suite_files`. The list view must see the same collection a run would: a per-file discover
/// skips the setup, so suite-level opts (a `spec` flag, `requires`, the suite name) silently
/// vanish — and a member file's `spec = false` marker reads as an orphan and errors. Caught by
/// the spec suites' own `--specs --list` (dogfooding).
pub(crate) fn discover_suite_files(
    name: &str,
    setup: Option<&Path>,
    files: &[PathBuf],
    config: &RunConfig,
) -> mlua::Result<Vec<String>> {
    if setup.is_none() && files.len() == 1 {
        return discover_path_with(&files[0], config);
    }
    let (lua, col) = build_lua(name.to_string(), config)?;
    if let Some(setup) = setup {
        let code = std::fs::read_to_string(setup).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", setup.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(setup)).exec()?;
    }
    load_member_files(&lua, &col, files)?;
    let col = col.borrow();
    list_plan(&col, config)
}

/// The shared tail of discovery: build the plan (validations included), honor selection and the
/// `--specs` filter, and return the surviving leaf paths.
fn list_plan(col: &Collector, config: &RunConfig) -> mlua::Result<Vec<String>> {
    let (plan, _deselected) =
        apply_selection(build_plan(col, &config.capabilities)?, &config.selection);
    let (plan, _spec_deselected) = apply_specs_filter(plan, config.specs_only);
    Ok(plan
        .leaves
        .iter()
        .flat_map(|leaf| leaf.unit.leaf_paths().into_iter().map(String::from))
        .collect())
}

/// A lint report for a plugin module: the grammar facets it exposes and any conformance issues.
/// What kind of namespace a plugin returned. A plugin is *any* Lua module that returns a table; the
/// resource shape (`client`/`container`/`wait_for`/`mock`) is one common kind, but a library of helpers is
/// equally valid — so lint classifies rather than requiring a fixed shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginShape {
    /// Exposes resource facets (`container`/`client`/`wait_for`/`mock`) — a provisioned, attachable, or
    /// virtualized resource.
    Resource,
    /// A table with no resource facets — a helper library (custom matchers, builders, DSLs, …).
    Library,
}

#[derive(Debug, Default)]
pub struct PluginReport {
    /// The plugin's shape, if it returned a table (`None` only when it returned a non-table).
    pub shape: Option<PluginShape>,
    /// Resource facet names found on the namespace (`client`/`container`/`wait_for`/`mock`). Empty for a
    /// library — which is fine, not an error.
    pub facets: Vec<String>,
    /// Conformance problems that make the plugin *invalid* — non-table return, or a malformed facet.
    /// An empty list means the plugin is well-formed (whatever its shape).
    pub issues: Vec<String>,
}

/// Load a plugin file (with the primitives + searcher installed, exactly as at run time), evaluate it
/// to its returned namespace, and check it against the plugin contract.
///
/// The *only* universal requirement is that a plugin `return`s a table. Beyond that, lint
/// **classifies** rather than prescribes: a namespace exposing resource facets
/// (`client`/`container`/`wait_for`) is a [`PluginShape::Resource`]; a plain table of helpers with no
/// such facets is a [`PluginShape::Library`] — equally valid. It therefore flags only what is wrong
/// for *any* plugin: a non-table return, or a resource facet that is present but not a function.
/// (A `container` facet is expected to yield the `{ client?, url, container }` trio, which can't be
/// verified without provisioning, so that is left to tests.)
pub fn inspect_plugin(path: &Path, config: &RunConfig) -> mlua::Result<PluginReport> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let (lua, _col) = build_lua("plugin".to_string(), config)?;
    let value: Value = lua.load(&code).set_name(file_chunk_name(path)).eval()?;

    let mut report = PluginReport::default();
    let Value::Table(ns) = value else {
        report.issues.push(format!(
            "plugin must `return` a namespace table, but returned a {}",
            value.type_name()
        ));
        return Ok(report);
    };

    // Recognized resource facets, in grammar order. A present facet must be a function; a malformed
    // one is an issue. Absent facets are fine — that just means this isn't a resource plugin.
    for facet in ["client", "container", "wait_for", "mock"] {
        match ns.get::<Value>(facet)? {
            Value::Nil => {}
            Value::Function(_) => report.facets.push(facet.to_string()),
            other => report.issues.push(format!(
                "`{facet}` should be a function, but is a {}",
                other.type_name()
            )),
        }
    }

    report.shape = Some(if report.facets.is_empty() {
        PluginShape::Library
    } else {
        PluginShape::Resource
    });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
