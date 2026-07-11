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
//! Execution defaults to **sequential** (`concurrency = 1`): correct and deterministic for
//! fixture-sharing tests. Parallelism is opt-in via `RunConfig`/`--jobs` and becomes safe once the
//! resource scheduler lands. Fixture factories are called synchronously in this increment (async
//! factories are a later step: the Lua API `ctx:use(handle)` is unchanged, only the Rust binding
//! upgrades from a sync to an async method).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::stream::StreamExt;
use mlua::{Function, Lua, UserData, UserDataMethods, Value};

use crate::model::{parse_duration, Event, NodeIx, Outcome, Params, Reporter, Summary, UnitOpts};

/// Throughput knob (never semantic). Defaults to sequential until the resource scheduler exists.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub concurrency: usize,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig { concurrency: 1 }
    }
}

// ---------------------------------------------------------------------------------------------
// Scopes & fixtures
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Test,
    File,
    Suite,
}

impl ScopeKind {
    fn rank(self) -> u8 {
        match self {
            ScopeKind::Test => 0,
            ScopeKind::File => 1,
            ScopeKind::Suite => 2,
        }
    }
    fn label(self) -> &'static str {
        match self {
            ScopeKind::Test => "test",
            ScopeKind::File => "file",
            ScopeKind::Suite => "suite",
        }
    }
    fn parse(s: &str) -> mlua::Result<Self> {
        match s {
            "test" => Ok(ScopeKind::Test),
            "file" => Ok(ScopeKind::File),
            "suite" => Ok(ScopeKind::Suite),
            other => Err(mlua::Error::RuntimeError(format!(
                "unknown fixture scope {other:?} (expected test|file|suite)"
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

fn teardown_scope(scope: &Rc<RefCell<ScopeState>>) {
    let (teardowns, tempdirs) = {
        let mut s = scope.borrow_mut();
        (
            std::mem::take(&mut s.teardowns),
            std::mem::take(&mut s.tempdirs),
        )
    };
    // LIFO: last registered runs first, so a fixture's cleanup runs before its dependencies'.
    for f in teardowns.into_iter().rev() {
        let _ = f.call::<()>(()); // TODO: surface teardown errors as findings
    }
    for dir in tempdirs.into_iter().rev() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn make_tempdir() -> std::io::Result<PathBuf> {
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
    Ok(UnitOpts { timeout, tags })
}

// ---------------------------------------------------------------------------------------------
// The context (`t` / `ctx`) — one type for test bodies and fixture factories
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct TestRun {
    assertions: usize,
    failure: Option<String>,
    skip: Option<String>,
}

/// Injected into every body/factory. `own_scope` is the scope its `defer`/`tempdir` target and the
/// floor for the scope-mismatch check; `test_scope` is the active test's scope instance.
struct Ctx {
    run: Rc<RefCell<TestRun>>,
    state: Rc<RunState>,
    test_scope: Rc<RefCell<ScopeState>>,
    own_scope: ScopeKind,
}

impl Ctx {
    fn scope_state(&self, kind: ScopeKind) -> Rc<RefCell<ScopeState>> {
        match kind {
            ScopeKind::Suite => self.state.suite.clone(),
            ScopeKind::File => self.state.file.clone(),
            ScopeKind::Test => self.test_scope.clone(),
        }
    }
    fn own_scope_state(&self) -> Rc<RefCell<ScopeState>> {
        self.scope_state(self.own_scope)
    }
}

fn resolve_use(lua: &Lua, this: &Ctx, target: Value) -> mlua::Result<Value> {
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

    let ss = this.scope_state(def.scope);
    if let Some(v) = ss.borrow().cache.get(&id) {
        return Ok(v.clone());
    }

    // Build lazily: a child context bound to the fixture's own scope.
    let child = Ctx {
        run: this.run.clone(),
        state: this.state.clone(),
        test_scope: this.test_scope.clone(),
        own_scope: def.scope,
    };
    let child_ud = lua.create_userdata(child)?;
    let value: Value = def.factory.call(child_ud)?;
    ss.borrow_mut().cache.insert(id, value.clone());
    Ok(value)
}

impl UserData for Ctx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("use", |lua, this, target: Value| resolve_use(lua, this, target));

        methods.add_method("defer", |_, this, f: Function| {
            this.own_scope_state().borrow_mut().teardowns.push(f);
            Ok(())
        });

        methods.add_method("tempdir", |_, this, ()| {
            let path = make_tempdir()
                .map_err(|e| mlua::Error::RuntimeError(format!("tempdir failed: {e}")))?;
            let s = path.to_string_lossy().into_owned();
            this.own_scope_state().borrow_mut().tempdirs.push(path);
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
        r.failure = Some(msg.clone());
        Err(mlua::Error::RuntimeError(msg))
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
                format!("expected {}, got {}", display(&other), display(&this.subject))
            })
        });
        methods.add_method("eq", |_, this, other: Value| {
            let pass = values_equal(&this.subject, &other);
            this.record(pass, || {
                format!("expected {}, got {}", display(&other), display(&this.subject))
            })
        });
        methods.add_method("is_true", |_, this, ()| {
            let pass = matches!(this.subject, Value::Boolean(true));
            this.record(pass, || format!("expected true, got {}", display(&this.subject)))
        });
        methods.add_method("is_false", |_, this, ()| {
            let pass = matches!(this.subject, Value::Boolean(false));
            this.record(pass, || format!("expected false, got {}", display(&this.subject)))
        });
        methods.add_method("is_nil", |_, this, ()| {
            let pass = matches!(this.subject, Value::Nil);
            this.record(pass, || format!("expected nil, got {}", display(&this.subject)))
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
        _ => false,
    }
}

fn contains(subject: &Value, needle: &Value) -> bool {
    match subject {
        Value::String(s) => match needle {
            Value::String(n) => s.to_string_lossy().contains(&*n.to_string_lossy()),
            _ => false,
        },
        Value::Table(t) => {
            for pair in t.clone().pairs::<Value, Value>() {
                if let Ok((_, v)) = pair {
                    if values_equal(&v, needle) {
                        return true;
                    }
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

fn build_lua(root_name: String) -> mlua::Result<(Lua, SharedCollector)> {
    let col: SharedCollector = Rc::new(RefCell::new(Collector::new(root_name)));
    let lua = Lua::new();
    let prova = lua.create_table()?;

    {
        let col = col.clone();
        prova.set(
            "test",
            lua.create_function(move |_, (name, a, b): (String, Value, Value)| {
                let (opts, body) = split_opts_body(a, b)?;
                col.borrow_mut().add(
                    0,
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
            })?,
        )?;
    }
    {
        let col = col.clone();
        prova.set(
            "group",
            lua.create_function(move |lua, (name, body): (String, Function)| {
                let gix = col.borrow_mut().add(
                    0,
                    Node {
                        name,
                        kind: NodeKind::Group,
                        params: Params::default(),
                        opts: UnitOpts::default(),
                        children: vec![],
                        body: None,
                    },
                );
                let gb = lua.create_userdata(GroupBuilder {
                    col: col.clone(),
                    ix: gix,
                })?;
                body.call::<()>(gb)?;
                Ok(())
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

    lua.globals().set("prova", prova)?;
    Ok((lua, col))
}

struct GroupBuilder {
    col: SharedCollector,
    ix: NodeIx,
}

impl UserData for GroupBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("test", |_, this, (name, a, b): (String, Value, Value)| {
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

        methods.add_method("group", |lua, this, (name, body): (String, Function)| {
            let gix = this.col.borrow_mut().add(
                this.ix,
                Node {
                    name,
                    kind: NodeKind::Group,
                    params: Params::default(),
                    opts: UnitOpts::default(),
                    children: vec![],
                    body: None,
                },
            );
            let gb = lua.create_userdata(GroupBuilder {
                col: this.col.clone(),
                ix: gix,
            })?;
            body.call::<()>(gb)?;
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

fn build_plan(col: &Collector, ix: NodeIx, ancestors: &mut Vec<String>, out: &mut Vec<PlanItem>) {
    let node = &col.nodes[ix];
    match node.kind {
        NodeKind::Group => {
            let named = ix != 0 && !node.name.is_empty();
            if named {
                ancestors.push(format!("{}{}", node.name, node.params.suffix()));
            }
            for &child in &node.children {
                build_plan(col, child, ancestors, out);
            }
            if named {
                ancestors.pop();
            }
        }
        NodeKind::Test => {
            let mut path = ancestors.clone();
            path.push(format!("{}{}", node.name, node.params.suffix()));
            out.push(PlanItem {
                path: path.join(" › "),
                body: node.body.clone().expect("test node has a body"),
                timeout: node.opts.timeout,
            });
        }
    }
}

struct NodeResult {
    path: String,
    outcome: Outcome,
    duration: Duration,
    assertions: usize,
    message: Option<String>,
}

async fn run_one(lua: &Lua, item: &PlanItem, state: &Rc<RunState>) -> NodeResult {
    let run = Rc::new(RefCell::new(TestRun::default()));
    let test_scope = Rc::new(RefCell::new(ScopeState::default()));
    let ctx = Ctx {
        run: run.clone(),
        state: state.clone(),
        test_scope: test_scope.clone(),
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
                teardown_scope(&test_scope); // teardown still runs after a timeout
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

    teardown_scope(&test_scope);

    NodeResult {
        path: item.path.clone(),
        outcome,
        duration,
        assertions,
        message,
    }
}

async fn run_plan(
    lua: &Lua,
    plan: &[PlanItem],
    state: &Rc<RunState>,
    config: &RunConfig,
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
) {
    for item in plan {
        reporter.event(&Event::NodeStarted { path: &item.path });
    }

    let mut stream = futures::stream::iter(plan.iter().map(|item| run_one(lua, item, state)))
        .buffer_unordered(config.concurrency.max(1));

    while let Some(result) = stream.next().await {
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

// ---------------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------------

fn read_and_collect(path: &Path) -> mlua::Result<(Lua, SharedCollector)> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tests")
        .to_string();
    let (lua, col) = build_lua(stem)?;
    lua.load(&code).set_name(path.to_string_lossy()).exec()?;
    Ok((lua, col))
}

fn new_runtime() -> mlua::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
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
    let (lua, col) = read_and_collect(path)?;
    let (plan, state) = {
        let col = col.borrow();
        let mut plan = Vec::new();
        build_plan(&col, 0, &mut Vec::new(), &mut plan);
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
        reporter.event(&Event::RunStarted);
        run_plan(&lua, &plan, &state, config, reporter, &mut summary).await;
        // Scopes tear down inner→outer: file, then suite (test scopes already torn down per-test).
        teardown_scope(&state.file);
        teardown_scope(&state.suite);
        summary.duration = started.elapsed();
        reporter.event(&Event::RunFinished { summary: &summary });
    });
    Ok(summary)
}

/// Discovery: collect the test tree without executing (basis for a GUI/IDE model view).
pub fn discover_path(path: &Path) -> mlua::Result<Vec<String>> {
    let (_lua, col) = read_and_collect(path)?;
    let col = col.borrow();
    let mut plan = Vec::new();
    build_plan(&col, 0, &mut Vec::new(), &mut plan);
    Ok(plan.into_iter().map(|item| item.path).collect())
}
