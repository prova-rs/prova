//! The engine: inject the `prova` global, collect a node tree, then execute it **asynchronously**.
//!
//! Async is foundational, not bolted on. Each test body is driven with `call_async`, so a body
//! can `await` I/O (HTTP, shell, sleep) without blocking a thread; a per-run current-thread
//! runtime drives many bodies **concurrently and cooperatively** on one Lua state. That single
//! decision unlocks three things at once:
//!   - real **timeouts** for I/O-bound hangs (cancel the future when the deadline elapses),
//!   - **I/O concurrency** at the scale load/stress testing needs (thousands of in-flight awaits),
//!   - a clean **definition → plan → execute** split so a future load driver reuses the same bodies.
//!
//! (CPU-bound Lua hangs still need an mlua interrupt hook — marked below. True multi-core
//! parallelism will use one Lua state per worker; this slice does cooperative single-state async.)

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::stream::StreamExt;
use mlua::{Function, Lua, UserData, UserDataMethods, Value};

use crate::model::{parse_duration, Event, NodeIx, Outcome, Params, Reporter, Summary, UnitOpts};

/// How the executor spends concurrency. `--jobs` maps here; it is *throughput only*, never
/// semantic (a flow is serial regardless; a group is parallelizable regardless).
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Max bodies polled concurrently on the (single) worker.
    pub concurrency: usize,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig { concurrency: 16 }
    }
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
// Lua-facing builders
// ---------------------------------------------------------------------------------------------

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
// Test context + assertions
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct TestRun {
    assertions: usize,
    failure: Option<String>,
    skip: Option<String>,
}

struct TestCtx {
    run: Rc<RefCell<TestRun>>,
}

const SKIP_SENTINEL: &str = "__prova_skip__";

impl UserData for TestCtx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
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

        methods.add_method("log", |_, _this, _msg: String| Ok(()));
    }
}

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
                Ok(()) // TODO: return a Test handle (seam for depends_on)
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

    // A minimal async primitive to prove the async spine end-to-end; real `http`/`shell`
    // modules land as async modules the same way.
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

async fn run_one(lua: &Lua, item: &PlanItem) -> NodeResult {
    let run = Rc::new(RefCell::new(TestRun::default()));
    // Fresh, injected context per invocation — the re-runnable-body seam (retries/shrink/load).
    let ctx = lua
        .create_userdata(TestCtx { run: run.clone() })
        .expect("create test context");

    let start = Instant::now();
    let call = item.body.call_async::<()>(ctx);

    // Deadline enforcement for the async (I/O) path: cancel the future when the budget elapses.
    // CPU-bound Lua hangs would additionally need an mlua interrupt hook (future increment).
    let result = match item.timeout {
        Some(budget) => match tokio::time::timeout(budget, call).await {
            Ok(r) => r,
            Err(_elapsed) => {
                return NodeResult {
                    path: item.path.clone(),
                    outcome: Outcome::Failed,
                    duration: start.elapsed(),
                    assertions: run.borrow().assertions,
                    message: Some(format!("timed out after {budget:?}")),
                };
            }
        },
        None => call.await,
    };

    let duration = start.elapsed();
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

    NodeResult {
        path: item.path.clone(),
        outcome,
        duration,
        assertions: r.assertions,
        message,
    }
}

async fn run_plan(
    lua: &Lua,
    plan: &[PlanItem],
    config: &RunConfig,
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
) {
    // Announce the known set up front (a frontend renders the tree before results arrive).
    for item in plan {
        reporter.event(&Event::NodeStarted { path: &item.path });
    }

    let mut stream = futures::stream::iter(plan.iter().map(|item| run_one(lua, item)))
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

/// Collect and run a test file with default config.
pub fn run_path(path: &Path, reporter: &mut dyn Reporter) -> mlua::Result<Summary> {
    run_path_with(path, reporter, &RunConfig::default())
}

/// Collect and run a test file with explicit config.
pub fn run_path_with(
    path: &Path,
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    let (lua, col) = read_and_collect(path)?;
    let col = col.borrow();
    let mut plan = Vec::new();
    build_plan(&col, 0, &mut Vec::new(), &mut plan);

    let rt = new_runtime()?;
    let mut summary = Summary::default();
    rt.block_on(async {
        let started = Instant::now();
        reporter.event(&Event::RunStarted);
        run_plan(&lua, &plan, config, reporter, &mut summary).await;
        summary.duration = started.elapsed();
        reporter.event(&Event::RunFinished { summary: &summary });
    });
    Ok(summary)
}

/// Discovery: collect the test tree **without executing** — the basis for a GUI/IDE that loads a
/// file, shows the tree, and runs selectively. Returns each test's full path.
pub fn discover_path(path: &Path) -> mlua::Result<Vec<String>> {
    let (_lua, col) = read_and_collect(path)?;
    let col = col.borrow();
    let mut plan = Vec::new();
    build_plan(&col, 0, &mut Vec::new(), &mut plan);
    Ok(plan.into_iter().map(|item| item.path).collect())
}
