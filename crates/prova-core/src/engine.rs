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
//! And **resources** (`prova.port`/`resource`/`shared`, `serial`): each leaf carries `reqs`, and a
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
        let has_includes = !self.keywords.is_empty() || !self.nodes.is_empty() || !self.tags.is_empty();
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

#[derive(Clone)]
pub struct RunConfig {
    pub concurrency: usize,
    /// Node selection applied after collection (empty = run everything).
    pub selection: Selection,
    modules: Vec<Module>,
    /// Extra disk roots the plugin searcher consults (e.g. the global `data_dir/plugins`).
    plugin_roots: Vec<std::path::PathBuf>,
    /// Manifest-declared plugins: name → an exact file (a local path, or a git checkout the CLI
    /// fetched into the cache). Authoritative over disk roots.
    named_plugins: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Plugin namespaces: a plugin's canonical name → its module root dir, so a multi-file plugin can
    /// `require("<canonical>.<sub>")` its own siblings.
    plugin_namespaces: std::collections::BTreeMap<String, std::path::PathBuf>,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            concurrency: 1,
            selection: Selection::default(),
            modules: Vec::new(),
            plugin_roots: Vec::new(),
            named_plugins: std::collections::BTreeMap::new(),
            plugin_namespaces: std::collections::BTreeMap::new(),
        }
    }
}

impl std::fmt::Debug for RunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunConfig")
            .field("concurrency", &self.concurrency)
            .field("selection", &self.selection)
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

    /// Register a plugin module — a `Fn(&Lua) -> Result<()>` run against every Lua state the run
    /// creates. Use this to inject domain globals (e.g. `prova_archetect::install`).
    pub fn with_module<F>(mut self, install: F) -> Self
    where
        F: Fn(&Lua) -> mlua::Result<()> + Send + Sync + 'static,
    {
        self.modules.push(std::sync::Arc::new(install));
        self
    }

    /// Add a disk root the plugin searcher consults (typically a `SystemLayout`'s `plugins_dir`).
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
    pub fn with_plugin_namespace(
        mut self,
        canonical: impl Into<String>,
        dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.plugin_namespaces.insert(canonical.into(), dir.into());
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

/// A typed resource reference from `prova.port`/`resource`/`shared`. Preferred over magic-format
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
}

/// Shared across the whole suite run: the fixture registry, the one suite-scope instance, and a lazy
/// **per-file** scope instance (a suite may load several files into one state, and each gets its own
/// `Scope.File`). A single file just has one entry (index 0).
struct RunState {
    defs: Vec<FixtureDef>,
    suite: Rc<RefCell<ScopeState>>,
    files: RefCell<HashMap<usize, Rc<RefCell<ScopeState>>>>,
}

impl RunState {
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
async fn teardown_scope(scope: &Rc<RefCell<ScopeState>>) {
    let (teardowns, tempdirs) = {
        let mut s = scope.borrow_mut();
        (
            std::mem::take(&mut s.teardowns),
            std::mem::take(&mut s.tempdirs),
        )
    };
    // LIFO: last registered runs first, so a fixture's cleanup runs before its dependencies'.
    for f in teardowns.into_iter().rev() {
        let _ = f.call_async::<()>(()).await; // TODO: surface teardown errors as findings
    }
    for dir in tempdirs.into_iter().rev() {
        let _ = std::fs::remove_dir_all(&dir);
    }
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
    /// The index of the file currently being loaded (a suite loads several files into one collector).
    /// Every node added while this is set records it, so `Scope.File` can reset per file.
    current_file: usize,
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
            }],
            fixtures: vec![],
            topologies: BTreeMap::new(),
            parent_stack: vec![0],
            current_file: 0,
        }
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
    Ok(UnitOpts {
        timeout,
        tags,
        depends_on,
        resources,
        serial,
        requires,
    })
}

/// A `resources` entry is a typed `ResourceRef` (exclusive or shared) or a bare string (an ad-hoc
/// exclusive token). Anything else is a helpful error rather than a silent no-op.
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
            .map_err(|_| {
                mlua::Error::RuntimeError(
                    "resources entries must be strings or prova.port/resource/shared refs".into(),
                )
            }),
        _ => Err(mlua::Error::RuntimeError(
            "resources entries must be strings or prova.port/resource/shared refs".into(),
        )),
    }
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

    let def = this.state.defs[id].clone();

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

struct Matcher {
    subject: Value,
    label: Option<String>,
    negated: bool,
    run: Rc<RefCell<TestRun>>,
}

impl Matcher {
    fn record(&self, raw_pass: bool, detail: impl FnOnce() -> String) -> mlua::Result<()> {
        let mut r = self.run.borrow_mut();
        r.assertions += 1;
        if raw_pass ^ self.negated {
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

impl UserData for Matcher {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("never", |lua, this, ()| {
            lua.create_userdata(Matcher {
                subject: this.subject.clone(),
                label: this.label.clone(),
                negated: !this.negated,
                run: this.run.clone(),
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
        methods.add_method("is_empty", |_, this, ()| {
            let pass = path_is_empty(&this.subject);
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

        // Lua-pattern match on a string subject (delegates to Lua's `string.find`).
        methods.add_method("matches", |lua, this, pattern: String| {
            let (pass, subject) = match &this.subject {
                Value::String(s) => {
                    let subject = s.to_str()?.to_string();
                    let find: mlua::Function = lua.globals().get::<Table>("string")?.get("find")?;
                    let found: Value = find.call((subject.clone(), pattern.clone()))?;
                    (!matches!(found, Value::Nil), subject)
                }
                other => (false, display(other)),
            };
            this.record(pass, || {
                format!("expected {subject:?} to match pattern {pattern:?}")
            })
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
        _ => false,
    }
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
                let parent = col.borrow().current_parent();
                let ix = register_test(&col, parent, name, a, b, None)?;
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
            lua.create_function(move |_lua, (label, body): (String, Function)| {
                register_describe(&col, label, body)
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
            let mut last_err: Option<String> = None;
            loop {
                match f.call_async::<Value>(()).await {
                    Ok(v) if truthy(&v) => return Ok(v),
                    Ok(_) => {}
                    Err(e) => last_err = Some(e.to_string()),
                }
                if Instant::now() >= deadline {
                    let base = message.unwrap_or_else(|| {
                        format!("prova.retry: condition not met within {timeout:?}")
                    });
                    return Err(mlua::Error::RuntimeError(match last_err {
                        Some(e) => format!("{base} (last error: {e})"),
                        None => base,
                    }));
                }
                tokio::time::sleep(every).await;
            }
        })?,
    )?;

    // Typed resource constructors. `port`/`resource` are exclusive; `shared` promotes any ref (or a
    // bare string token) to a concurrent reader.
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
        "resource",
        lua.create_function(|lua, token: String| {
            lua.create_userdata(ResourceRef {
                token,
                shared: false,
            })
        })?,
    )?;
    prova.set(
        "shared",
        lua.create_function(|lua, v: Value| {
            let req = parse_resource(v)?;
            lua.create_userdata(ResourceRef {
                token: req.token,
                shared: true,
            })
        })?,
    )?;

    lua.globals().set("prova", prova)?;

    // The typed fixture-scope constants: `Scope.Test` / `Scope.Flow` / `Scope.File` / `Scope.Suite`.
    lua.globals().set("Scope", make_scope_global(&lua)?)?;

    // `suite.config{ name?, requires? }` — configure the current suite (used in a `suite.lua` setup
    // file). `requires` gates the whole suite: it folds into the root node so every test inherits it,
    // and an unmet capability skips all the suite's files cleanly (skip, not fail).
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
            let ix = register_test(&this.col, this.ix, name, a, b, None)?;
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

/// Register a leaf `test`/`step` node under `parent`; returns its arena index (the unit handle id).
/// `case` is the `test_each` case value (`None` for an ordinary test).
fn register_test(
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
    case: Option<Value>,
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
        },
    );
    let gb = lua.create_userdata(GroupBuilder {
        col: col.clone(),
        ix: gix,
    })?;
    body.call::<()>(gb)?;
    Ok(gix)
}

/// Register a `describe` labeling group under the current ambient parent, then run its body with
/// that group pushed on the parent stack so **bare** `prova.test`/`test_each`/`group`/`flow` inside
/// the body nest under the label (dynamic scoping). Structurally a group — labeling only, no new
/// fixture scope. The stack is popped even if the body errors, so one bad `describe` can't corrupt
/// the ambient parent for the rest of the file.
fn register_describe(col: &SharedCollector, label: String, body: Function) -> mlua::Result<()> {
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
        },
    );
    let fb = lua.create_userdata(FlowBuilder {
        col: col.clone(),
        ix: fix,
    })?;
    body.call::<()>(fb)?;
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
        methods.add_method("step", |_, this, (name, a, b): (String, Value, Value)| {
            let (opts, body) = split_opts_body(a, b)?;
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

fn build_plan(col: &Collector) -> mlua::Result<Plan> {
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
    resolve_requires(&mut leaves);
    Ok(Plan { leaves })
}

/// Set `precondition_skip` on any leaf whose `requires` include an unavailable capability.
fn resolve_requires(leaves: &mut [Leaf]) {
    let mut cache: HashMap<String, bool> = HashMap::new();
    for leaf in leaves.iter_mut() {
        for cap in &leaf.requires {
            let available = *cache
                .entry(cap.clone())
                .or_insert_with(|| capability_available(cap));
            if !available {
                leaf.precondition_skip = Some(format!("skipped: requires {cap:?} (unavailable)"));
                break;
            }
        }
    }
}

/// Is a capability available on this host? Known capabilities have detectors; anything else is
/// treated as "a tool of that name on PATH" (so `requires = { "kubectl" }` just works). A missing
/// capability never fails a test — it skips it, visibly.
fn capability_available(name: &str) -> bool {
    match name {
        // The docker daemon must be reachable *and* the feature compiled in. Retry a few times: a
        // single `docker info` can transiently fail when the daemon is momentarily busy (heavy
        // container churn — e.g. many container tests tearing down at once), which would otherwise
        // skip a whole test spuriously. This resolves once per run (memoized), so the cost is bounded;
        // a genuinely-absent daemon fails fast (connection-refused is instant), so the retry budget is
        // paid mostly as backoff sleeps only when the daemon is present-but-busy.
        "docker" => cfg!(feature = "docker") && command_succeeds_retry("docker", &["info"], 8),
        "github" => std::env::var_os("GITHUB_TOKEN").is_some(),
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
}

async fn run_one(
    lua: &Lua,
    item: &PlanItem,
    state: &Rc<RunState>,
    flow_scope: Option<Rc<RefCell<ScopeState>>>,
) -> NodeResult {
    let run = Rc::new(RefCell::new(TestRun::default()));
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
    };
    let ctx_ud = lua.create_userdata(ctx).expect("create context");

    let start = Instant::now();
    let call = item.body.call_async::<()>((ctx_ud, case_arg));

    let result = match item.timeout {
        Some(budget) => match tokio::time::timeout(budget, call).await {
            Ok(r) => r,
            Err(_elapsed) => {
                let assertions = run.borrow().assertions;
                teardown_scope(&test_scope).await; // teardown still runs after a timeout
                return NodeResult {
                    path: item.path.clone(),
                    outcome: Outcome::Failed,
                    duration: start.elapsed(),
                    assertions,
                    message: Some(format!("timed out after {budget:?}")),
                };
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

    teardown_scope(&test_scope).await;

    NodeResult {
        path: item.path.clone(),
        outcome,
        duration,
        assertions,
        message,
    }
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
            });
            continue;
        }
        let result = run_one(lua, step, state, Some(flow_scope.clone())).await;
        if result.outcome == Outcome::Failed {
            let failed = step_name(&step.path);
            cascade = Some(format!("skipped: earlier step {failed:?} failed"));
        }
        results.push(result);
    }

    teardown_scope(&flow_scope).await;
    results
}

/// The last path segment — the step's own name, for the cascade-skip message.
fn step_name(path: &str) -> &str {
    path.rsplit(" › ").next().unwrap_or(path)
}

async fn run_unit(lua: &Lua, unit: &PlanUnit, state: &Rc<RunState>) -> Vec<NodeResult> {
    match unit {
        PlanUnit::Test(item) => vec![run_one(lua, item, state, None).await],
        PlanUnit::Flow { steps } => run_flow(lua, steps, state).await,
    }
}

/// The unit-level outcome used for dependency gating: a unit failed if any of its leaf results
/// failed; else passed if any passed; else it was entirely skipped.
fn unit_outcome(results: &[NodeResult]) -> Outcome {
    if results.iter().any(|r| r.outcome == Outcome::Failed) {
        Outcome::Failed
    } else if results.iter().any(|r| r.outcome == Outcome::Passed) {
        Outcome::Passed
    } else {
        Outcome::Skipped
    }
}

/// Build skipped results for a unit that never ran (a dependency did not pass) — one per reported
/// path (a flow reports one skip per step), so the report stays consistent with a unit that ran.
fn skip_leaf(unit: &PlanUnit, reason: &str) -> Vec<NodeResult> {
    unit.leaf_paths()
        .into_iter()
        .map(|path| NodeResult {
            path: path.to_string(),
            outcome: Outcome::Skipped,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some(reason.to_string()),
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
                    let results = skip_leaf(&leaves[i].unit, &reason);
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

        let (i, results) = in_flight.next().await.expect("in_flight is non-empty");
        resources.release(&leaves[i].reqs);
        outcome[i] = Some(unit_outcome(&results));
        emit_finished(reporter, summary, &results);
    }
}

// ---------------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------------

fn read_and_collect(path: &Path, config: &RunConfig) -> mlua::Result<(Lua, SharedCollector)> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tests")
        .to_string();
    let (lua, col) = build_lua(stem, config)?;
    lua.load(&code).set_name(path.to_string_lossy()).exec()?;
    Ok((lua, col))
}

fn new_runtime() -> mlua::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all() // time (timeouts/sleep) + io (child-process pipes for the shell module)
        .build()
        .map_err(|e| mlua::Error::RuntimeError(format!("failed to start async runtime: {e}")))
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
        lua.load(&code).set_name(setup.to_string_lossy()).exec()?;
    }

    // Each member file loads under a file-group node, with its own file index (1-based).
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
            let fg = c.add(0, group_node(stem));
            c.parent_stack.push(fg);
        }
        let code = std::fs::read_to_string(file).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", file.display()))
        })?;
        lua.load(&code).set_name(file.to_string_lossy()).exec()?;
        {
            let mut c = col.borrow_mut();
            c.parent_stack.pop();
            c.current_file = 0;
        }
    }

    execute_collected(&lua, &col, reporter, config)
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
        let (plan, deselected) = apply_selection(build_plan(&col)?, &config.selection);
        let state = Rc::new(RunState {
            defs: col.fixtures.clone(),
            suite: Rc::new(RefCell::new(ScopeState::default())),
            files: RefCell::new(HashMap::new()),
        });
        (plan, deselected, state)
    };

    let rt = new_runtime()?;
    let mut summary = Summary::default();
    summary.deselected = deselected;
    rt.block_on(async {
        let started = Instant::now();
        run_plan(lua, &plan, &state, config, reporter, &mut summary).await;
        // Scopes tear down inner→outer: every file scope, then the suite (test scopes already torn
        // down per-test).
        teardown_file_scopes(&state).await;
        teardown_scope(&state.suite).await;
        summary.duration = started.elapsed();
    });
    Ok(summary)
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
    let (lua, col) = build_lua("up".to_string(), config)?;
    for file in files {
        let code = std::fs::read_to_string(file).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", file.display()))
        })?;
        lua.load(&code).set_name(file.to_string_lossy()).exec()?;
    }

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
    });

    let rt = new_runtime()?;
    rt.block_on(async {
        let result = provision_and_hold(&lua, &state, id, name, on_ready).await;
        // Always tear down whatever got provisioned — a clean signal, or a mid-provision failure.
        teardown_file_scopes(&state).await;
        teardown_scope(&state.suite).await;
        result
    })
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
    let file0 = state.file_scope(0);
    let ctx = Ctx {
        run: Rc::new(RefCell::new(TestRun::default())),
        state: state.clone(),
        test_scope: file0.clone(), // no test scope in `up`; the File scope stands in for `manage`
        file_scope: file0,
        flow_scope: None,
        own_scope: ScopeKind::File,
        case: None,
    };
    let handle = lua.create_userdata(FixtureHandle { id })?;
    let value = resolve_use(lua, &ctx, Value::UserData(handle)).await?;
    let endpoints = extract_endpoints(&value, topo_name);
    on_ready(&endpoints);
    wait_for_shutdown().await;
    Ok(())
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
    }
}

/// Tear down every per-file `Scope.File` instance (a suite may have several).
async fn teardown_file_scopes(state: &RunState) {
    let scopes: Vec<_> = state.files.borrow().values().cloned().collect();
    for scope in scopes {
        teardown_scope(&scope).await;
    }
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
    let (plan, _deselected) = apply_selection(build_plan(&col)?, &config.selection);
    Ok(plan
        .leaves
        .iter()
        .flat_map(|leaf| leaf.unit.leaf_paths().into_iter().map(String::from))
        .collect())
}

/// A lint report for a plugin module: the grammar facets it exposes and any conformance issues.
/// What kind of namespace a plugin returned. A plugin is *any* Lua module that returns a table; the
/// resource shape (`client`/`container`/`wait_for`) is one common kind, but a library of helpers is
/// equally valid — so lint classifies rather than requiring a fixed shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginShape {
    /// Exposes resource facets (`container`/`client`/`wait_for`) — a provisioned/attachable resource.
    Resource,
    /// A table with no resource facets — a helper library (custom matchers, builders, DSLs, …).
    Library,
}

#[derive(Debug, Default)]
pub struct PluginReport {
    /// The plugin's shape, if it returned a table (`None` only when it returned a non-table).
    pub shape: Option<PluginShape>,
    /// Resource facet names found on the namespace (`client`/`container`/`wait_for`). Empty for a
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
    let value: Value = lua.load(&code).set_name(path.to_string_lossy()).eval()?;

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
    for facet in ["client", "container", "wait_for"] {
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
