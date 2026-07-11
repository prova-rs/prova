//! The engine: inject the `prova` global, collect a node tree, then execute it.
//!
//! Design seams honored here (features come later, the shape does not):
//!  - **definition ≠ execution**: `Node`s are collected first; a separate pass *runs* them, so a
//!    different driver (load/stress) could run the same bodies differently.
//!  - **re-runnable, context-injected bodies**: a test body is an `mlua::Function` we can call
//!    with a freshly-built context — the precondition for retries, shrinking, and load loops.
//!  - **params in identity**: every node carries `Params` (empty for now) folded into its path.
//!  - **deadline seam**: `UnitOpts::timeout` is parsed and carried; where enforcement will hook
//!    in is marked in `run_node`.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, UserData, UserDataMethods, Value};

use crate::model::{
    parse_duration, Event, NodeIx, Outcome, Params, Reporter, Summary, UnitOpts,
};

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
    #[allow(dead_code)] // carried for selection/reporting increments
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

/// Accept either `(name, fn)` or `(name, opts, fn)`.
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

/// The builder handed to a `prova.group(name, function(g) ... end)` body. It exposes `test`/
/// `group` — and deliberately no shared-state mechanism (see the group/flow design).
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
// Test context + assertions (injected into each test body)
// ---------------------------------------------------------------------------------------------

/// Accumulates what happened during one test invocation. Interior-mutable so `t`/matchers can
/// record while the body runs; read back by the executor afterward.
#[derive(Default)]
struct TestRun {
    assertions: usize,
    failure: Option<String>,
    skip: Option<String>,
}

/// The `t` handed to a test body. (Colon-methods, matching the DSL: `t:expect`, `t:skip`.)
struct TestCtx {
    run: Rc<RefCell<TestRun>>,
}

const SKIP_SENTINEL: &str = "__prova_skip__";

impl UserData for TestCtx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("expect", |lua, this, (subject, label): (Value, Option<String>)| {
            lua.create_userdata(Matcher {
                subject,
                label,
                negated: false,
                run: this.run.clone(),
            })
        });

        methods.add_method("skip", |_, this, reason: String| -> mlua::Result<()> {
            this.run.borrow_mut().skip = Some(reason);
            Err(mlua::Error::RuntimeError(SKIP_SENTINEL.into()))
        });

        // No-op in the POC; kept so bodies can call it without erroring.
        methods.add_method("log", |_, _this, _msg: String| Ok(()));
    }
}

/// A fluent matcher. Terminal matchers record into `TestRun` and raise on failure (hard assert).
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
        let prefix = self.label.as_ref().map(|l| format!("{l}: ")).unwrap_or_default();
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
                format!("expected {} to contain {}", display(&this.subject), display(&needle))
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
// Collection + execution
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
                    0, // the file's implicit root group
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

    // Injected as a global — no `require` needed.
    lua.globals().set("prova", prova)?;
    Ok((lua, col))
}

#[allow(clippy::too_many_arguments)]
fn run_node(
    col: &Collector,
    ix: NodeIx,
    lua: &Lua,
    ancestors: &mut Vec<String>,
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
) {
    let node = &col.nodes[ix];
    match node.kind {
        NodeKind::Group => {
            let is_named = node.name.is_empty() == false && ix != 0;
            if is_named {
                ancestors.push(format!("{}{}", node.name, node.params.suffix()));
            }
            for &child in &node.children {
                run_node(col, child, lua, ancestors, reporter, summary);
            }
            if is_named {
                ancestors.pop();
            }
        }
        NodeKind::Test => {
            let mut path = ancestors.clone();
            path.push(format!("{}{}", node.name, node.params.suffix()));
            let path = path.join(" › ");

            reporter.event(&Event::NodeStarted { path: &path });

            let run = Rc::new(RefCell::new(TestRun::default()));
            // Fresh context per invocation — the re-runnable-body seam.
            let ctx = lua
                .create_userdata(TestCtx { run: run.clone() })
                .expect("create test context");
            let body = node.body.clone().expect("test node has a body");

            // Deadline seam: `node.opts.timeout` would arm an mlua interrupt hook +
            // deadline-aware I/O here in a later increment.
            let start = Instant::now();
            let result = body.call::<()>(ctx);
            let duration = start.elapsed();

            let run = run.borrow();
            let (outcome, message) = if run.skip.is_some() {
                (Outcome::Skipped, run.skip.clone())
            } else if let Err(err) = &result {
                let msg = run
                    .failure
                    .clone()
                    .unwrap_or_else(|| lua_error_message(err));
                (Outcome::Failed, Some(msg))
            } else {
                (Outcome::Passed, None)
            };

            summary.tally(outcome);
            reporter.event(&Event::NodeFinished {
                path: &path,
                outcome,
                duration,
                assertions: run.assertions,
                message: message.as_deref(),
            });
        }
    }
}

/// Turn a raw Lua error (e.g. a runtime error thrown by non-assertion code) into a message.
fn lua_error_message(err: &mlua::Error) -> String {
    let s = err.to_string();
    if s.contains(SKIP_SENTINEL) {
        // Shouldn't reach here (skip is handled via the run flag), but be defensive.
        "skipped".into()
    } else {
        s
    }
}

/// Collect and run a Lua test file, driving `reporter` with the event stream.
pub fn run_path(path: &Path, reporter: &mut dyn Reporter) -> mlua::Result<Summary> {
    let code = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("cannot read {}: {e}", path.display())))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tests")
        .to_string();

    let (lua, col) = build_lua(stem)?;
    // Collection phase: running the file registers nodes into the arena.
    lua.load(&code).set_name(path.to_string_lossy()).exec()?;

    // Execution phase.
    let col = col.borrow();
    let mut summary = Summary::default();
    let started = Instant::now();
    reporter.event(&Event::RunStarted);
    let mut ancestors = Vec::new();
    run_node(&col, 0, &lua, &mut ancestors, reporter, &mut summary);
    summary.duration = started.elapsed();
    reporter.event(&Event::RunFinished { summary: &summary });

    let _ = Duration::ZERO; // keep Duration import used regardless of feature flags
    Ok(summary)
}
