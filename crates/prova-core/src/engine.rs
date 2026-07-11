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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::stream::StreamExt;
use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};

use crate::model::{
    parse_duration, Event, NodeIx, Outcome, Params, Reporter, ResourceReq, Summary, UnitOpts,
};

/// Throughput knob (never semantic). Defaults to sequential until the resource scheduler exists.
/// A plugin module: registers extra globals (e.g. an `archetect` table) into a fresh Lua state.
/// Called once per state, on worker threads, so it must be `Send + Sync`. Built-in modules
/// (`shell`, `fs`) are always installed; these are added by the host (CLI / an integration crate),
/// keeping `prova-core` domain-agnostic.
pub type Module = std::sync::Arc<dyn Fn(&Lua) -> mlua::Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct RunConfig {
    pub concurrency: usize,
    modules: Vec<Module>,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            concurrency: 1,
            modules: Vec::new(),
        }
    }
}

impl std::fmt::Debug for RunConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunConfig")
            .field("concurrency", &self.concurrency)
            .field("modules", &self.modules.len())
            .finish()
    }
}

impl RunConfig {
    pub fn new(concurrency: usize) -> Self {
        RunConfig {
            concurrency,
            modules: Vec::new(),
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
    fn parse(s: &str) -> mlua::Result<Self> {
        match s {
            "test" => Ok(ScopeKind::Test),
            "flow" => Ok(ScopeKind::Flow),
            "file" => Ok(ScopeKind::File),
            "suite" => Ok(ScopeKind::Suite),
            other => Err(mlua::Error::RuntimeError(format!(
                "unknown fixture scope {other:?} (expected test|flow|file|suite)"
            ))),
        }
    }
}

fn parse_scope(v: Value) -> mlua::Result<ScopeKind> {
    match v {
        Value::String(s) => ScopeKind::parse(&s.to_string_lossy()),
        Value::Table(t) => ScopeKind::parse(&t.get::<String>("scope")?),
        _ => Err(mlua::Error::RuntimeError(
            "fixture scope must be a string or an options table".into(),
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

/// Shared across the whole run: the fixture registry plus the suite & file scope instances.
struct RunState {
    defs: Vec<FixtureDef>,
    suite: Rc<RefCell<ScopeState>>,
    file: Rc<RefCell<ScopeState>>,
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
}

struct Collector {
    nodes: Vec<Node>,
    fixtures: Vec<FixtureDef>,
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
            }],
            fixtures: vec![],
        }
    }

    fn add(&mut self, parent: NodeIx, node: Node) -> NodeIx {
        let ix = self.nodes.len();
        self.nodes.push(node);
        self.nodes[parent].children.push(ix);
        ix
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
    flow_scope: Option<Rc<RefCell<ScopeState>>>,
    own_scope: ScopeKind,
}

impl Ctx {
    fn scope_state(&self, kind: ScopeKind) -> mlua::Result<Rc<RefCell<ScopeState>>> {
        Ok(match kind {
            ScopeKind::Suite => self.state.suite.clone(),
            ScopeKind::File => self.state.file.clone(),
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
        flow_scope: this.flow_scope.clone(),
        own_scope: def.scope,
    };
    let child_ud = lua.create_userdata(child)?;
    let value: Value = def.factory.call_async(child_ud).await?;
    ss.borrow_mut().cache.insert(id, value.clone());
    Ok(value)
}

impl UserData for Ctx {
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

fn build_lua(root_name: String, modules: &[Module]) -> mlua::Result<(Lua, SharedCollector)> {
    let col: SharedCollector = Rc::new(RefCell::new(Collector::new(root_name)));
    let lua = Lua::new();
    let prova = lua.create_table()?;

    {
        let col = col.clone();
        prova.set(
            "test",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                let ix = register_test(&col, 0, name, a, b)?;
                lua.create_userdata(UnitHandle { ix })
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "group",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                let ix = register_group(lua, &col, 0, name, a, b)?;
                lua.create_userdata(UnitHandle { ix })
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "flow",
            lua.create_function(move |lua, (name, a, b): (String, Value, Value)| {
                let ix = register_flow(lua, &col, 0, name, a, b)?;
                lua.create_userdata(UnitHandle { ix })
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

    prova.set(
        "sleep",
        lua.create_async_function(|_, millis: u64| async move {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            Ok(())
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

    // First-party capability modules (`shell`, `fs`) as their own injected globals.
    crate::modules::install(&lua)?;

    // Host-provided plugin modules (e.g. `archetect`), installed into every Lua state.
    for install in modules {
        install(&lua)?;
    }

    Ok((lua, col))
}

struct GroupBuilder {
    col: SharedCollector,
    ix: NodeIx,
}

impl UserData for GroupBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("test", |lua, this, (name, a, b): (String, Value, Value)| {
            let ix = register_test(&this.col, this.ix, name, a, b)?;
            lua.create_userdata(UnitHandle { ix })
        });

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
    }
}

/// Register a leaf `test`/`step` node under `parent`; returns its arena index (the unit handle id).
fn register_test(
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
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
        },
    ))
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
        },
    );
    let gb = lua.create_userdata(GroupBuilder {
        col: col.clone(),
        ix: gix,
    })?;
    body.call::<()>(gb)?;
    Ok(gix)
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
}

/// Group-level options that flow down to every contained leaf: `depends_on`, `resources`, `serial`,
/// `requires`.
#[derive(Clone, Default)]
struct Inherited {
    deps: Vec<NodeIx>,
    resources: Vec<ResourceReq>,
    serial: bool,
    requires: Vec<String>,
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
    let id = leaves.len();
    leaves.push(Leaf {
        unit,
        raw_deps,
        deps: Vec::new(),
        reqs,
        serial: inherited.serial || node.opts.serial,
        requires,
        precondition_skip: None,
    });
    id
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
        // The docker daemon must be reachable, not just the client installed.
        "docker" => command_succeeds("docker", &["info"]),
        "github" => std::env::var_os("GITHUB_TOKEN").is_some(),
        // No cheap, reliable synchronous probe; assume present (a real offline mode is future work).
        "network" | "internet" => true,
        other => binary_on_path(other),
    }
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

/// Is an executable named `name` on `PATH`?
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
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
    let ctx = Ctx {
        run: run.clone(),
        state: state.clone(),
        test_scope: test_scope.clone(),
        flow_scope,
        own_scope: ScopeKind::Test,
    };
    let ctx_ud = lua.create_userdata(ctx).expect("create context");

    let start = Instant::now();
    let call = item.body.call_async::<()>(ctx_ud);

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

fn read_and_collect(path: &Path, modules: &[Module]) -> mlua::Result<(Lua, SharedCollector)> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tests")
        .to_string();
    let (lua, col) = build_lua(stem, modules)?;
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
    let (lua, col) = read_and_collect(path, &config.modules)?;
    let (plan, state) = {
        let col = col.borrow();
        let plan = build_plan(&col)?;
        let state = Rc::new(RunState {
            defs: col.fixtures.clone(),
            suite: Rc::new(RefCell::new(ScopeState::default())),
            file: Rc::new(RefCell::new(ScopeState::default())),
        });
        (plan, state)
    };

    let rt = new_runtime()?;
    let mut summary = Summary::default();
    rt.block_on(async {
        let started = Instant::now();
        run_plan(&lua, &plan, &state, config, reporter, &mut summary).await;
        // Scopes tear down inner→outer: file, then suite (test scopes already torn down per-test).
        teardown_scope(&state.file).await;
        teardown_scope(&state.suite).await;
        summary.duration = started.elapsed();
    });
    Ok(summary)
}

/// Discovery: collect the test tree without executing (basis for a GUI/IDE model view).
pub fn discover_path(path: &Path) -> mlua::Result<Vec<String>> {
    // Discovery only needs the built-in globals; plugin modules are for execution.
    let (_lua, col) = read_and_collect(path, &[])?;
    let col = col.borrow();
    let plan = build_plan(&col)?;
    Ok(plan
        .leaves
        .iter()
        .flat_map(|leaf| leaf.unit.leaf_paths().into_iter().map(String::from))
        .collect())
}
