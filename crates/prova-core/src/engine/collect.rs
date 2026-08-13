//! The collection model: the definition tree (Node), the Collector behind the
//! registration surface, and the option/flag parsers.

use super::*;

// ---------------------------------------------------------------------------------------------
// Collection model
// ---------------------------------------------------------------------------------------------

pub(super) enum NodeKind {
    Group,
    Flow,
    Test,
}

pub(super) struct Node {
    pub(super) name: String,
    pub(super) kind: NodeKind,
    pub(super) params: Params,
    pub(super) opts: UnitOpts,
    pub(super) children: Vec<NodeIx>,
    pub(super) body: Option<Function>,
    /// The declared mutation that must turn `body` red (`falsified_by`). Lives beside the body
    /// rather than in `UnitOpts` because it is a Lua value, and `UnitOpts` stays plain data.
    pub(super) falsifier: Option<Function>,
    /// A `test_each` case, delivered to the body as its second argument and as `t.case`. `None` for
    /// ordinary tests (the body simply ignores the extra nil argument).
    pub(super) case: Option<Value>,
    /// Index of the source file this node was collected from (a suite may load several files into one
    /// state). Set by `Collector::add`; drives per-file `Scope.File`. Always 0 for a single file.
    pub(super) file: usize,
    /// 1-based line of the declaration call (`prova.test(...)`, `flow:step(...)`) in that file,
    /// captured from the Lua stack at registration. `None` when no frame in the current file was
    /// on the stack (a synthetic node, or a declaration made entirely from a helper chunk).
    pub(super) line: Option<u32>,
}

/// One `prova.remind(name, { when = fn }, message)` declaration, as collected. Not a node: a
/// reminder never enters the plan, the selection, or the tally — it is harvested by the
/// attention-account pass ([`evaluate_reminders`]) after the proofs complete.
pub(super) struct ReminderDef {
    pub(super) name: String,
    /// The trigger — plain Lua over the same primitives tests use. Receives the run's account.
    pub(super) when: Function,
    /// The instruction: what to DO when this fires (the discharge is an act, not an assertion).
    pub(super) message: String,
    /// Capability expressions gating evaluation, `requires`-style. Unmet → `Unevaluated`, never
    /// `Quiet` — a tripwire that could not look must not report that it saw nothing.
    pub(super) requires: Vec<String>,
    /// Free-form tags, the same grammar tests use — so a reminder is addressable by name OR tag when
    /// a context chooses which DUE reminders to heed (`--heed <selector>`, a profile's `heed` list).
    pub(super) tags: Vec<String>,
    pub(super) file: usize,
    pub(super) line: Option<u32>,
}

pub(super) struct Collector {
    pub(super) nodes: Vec<Node>,
    pub(super) fixtures: Vec<FixtureDef>,
    /// Reminders declared while loading (`prova.remind`). Collected beside the nodes, reported in a
    /// separate account — see docs/design/reminders.md.
    pub(super) reminders: Vec<ReminderDef>,
    /// Named topologies (`prova.topology`) → their fixture id, so `prova up <name>` can address a
    /// whole environment by name. A topology is a fixture that is *also* addressable by the `up`/
    /// `start` verbs; in test mode it is used exactly like any other fixture (`t:use(handle)`).
    pub(super) topologies: BTreeMap<String, usize>,
    /// The stack of ambient parents for *bare* top-level declarations (`prova.test`/`test_each`/
    /// `group`/`flow`). `prova.describe` pushes its labeling group so bare declarations inside its
    /// body nest under it (dynamic scoping); everything pops back to the file root (index 0).
    pub(super) parent_stack: Vec<NodeIx>,
    /// How many `group`/`flow` builder bodies are currently executing. Non-zero means bare
    /// declarations are a misuse (they would silently register at the file root, outside the unit
    /// being built) — children belong on the builder (`g:test`/`flow:step`), so the bare forms
    /// error instead of registering somewhere the author did not mean.
    pub(super) builder_depth: usize,
    /// The index of the file currently being loaded (a suite loads several files into one collector).
    /// Every node added while this is set records it, so `Scope.File` can reset per file.
    pub(super) current_file: usize,
    /// True when this state loads ONE ungrouped file — a singleton suite. `Scope.Suite` there is
    /// legal but behaves as `Scope.File`, which is almost never what the author meant; fixture
    /// registration warns, naming the fix (docs/plans/shared-deputies.md).
    pub(super) singleton_suite: bool,
    /// Source path per file index (`file_paths[i]` is the file loaded as index `i`), so a snapshot
    /// assertion can colocate its `.snap` beside the test file it ran from. Grown as files load.
    pub(super) file_paths: Vec<PathBuf>,
}

impl Collector {
    pub(super) fn new(root_name: String) -> Self {
        Collector {
            nodes: vec![Node {
                name: root_name,
                kind: NodeKind::Group,
                params: Params::default(),
                opts: UnitOpts::default(),
                children: vec![],
                body: None,
                falsifier: None,
                case: None,
                file: 0,
                line: None,
            }],
            fixtures: vec![],
            reminders: vec![],
            topologies: BTreeMap::new(),
            parent_stack: vec![0],
            builder_depth: 0,
            current_file: 0,
            singleton_suite: false,
            file_paths: Vec::new(),
        }
    }

    /// Record the source path for a file index (idempotent-ish: grows the vec so `file_paths[idx]` is
    /// set). Called as each file loads, before its nodes are collected.
    pub(super) fn set_file_path(&mut self, idx: usize, path: &Path) {
        if self.file_paths.len() <= idx {
            self.file_paths.resize(idx + 1, PathBuf::new());
        }
        self.file_paths[idx] = path.to_path_buf();
    }

    pub(super) fn add(&mut self, parent: NodeIx, mut node: Node) -> NodeIx {
        node.file = self.current_file; // stamp every node with the file being loaded
        let ix = self.nodes.len();
        self.nodes.push(node);
        self.nodes[parent].children.push(ix);
        ix
    }

    /// The current ambient parent for a bare top-level declaration.
    pub(super) fn current_parent(&self) -> NodeIx {
        *self.parent_stack.last().unwrap_or(&0)
    }
}

/// Reject a bare declaration (`prova.test`/`test_each`/`group`/`flow`/`describe`) made while a
/// `group`/`flow` builder body is executing. The bare form would register at the ambient parent —
/// the file root — not inside the unit being built, so the flow would run zero of "its" steps and
/// the tests would lose the parent's ordering/opts. Silently-wrong structure; error instead.
pub(super) fn reject_bare_in_builder(col: &SharedCollector, what: &str) -> mlua::Result<()> {
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

pub(super) type SharedCollector = Rc<RefCell<Collector>>;

pub(super) fn split_opts_body(
    a: Value,
    b: Value,
    kind: &str,
    unit: &str,
) -> mlua::Result<(UnitOpts, Function, Option<Function>)> {
    match (a, b) {
        (Value::Function(f), Value::Nil) => Ok((UnitOpts::default(), f, None)),
        (Value::Table(t), Value::Function(f)) => {
            reject_unknown_opts(&t, UNIT_OPTS, &format!("prova.{kind}(\"{unit}\")"))?;
            let falsifier = parse_falsified_by(&t.get::<Value>("falsified_by")?)?;
            Ok((parse_opts(&t)?, f, falsifier))
        }
        _ => Err(mlua::Error::RuntimeError(
            "expected (name, fn) or (name, opts, fn)".into(),
        )),
    }
}

/// Every option a unit's `opts` table may carry — closed by construction. `resources` is the
/// deprecated spelling of `locks`: accepted (it warns in `parse_opts`) but deliberately not
/// advertised, so the message never teaches a spelling on its way out.
pub(super) const UNIT_OPTS: &[&str] = &[
    "covers",
    "depends_on",
    "falsified_by",
    "locks",
    "promises",
    "proves",
    "requires",
    "resources",
    "serial",
    "switch",
    "tags",
    "timeout",
];

/// Spellings prova USED to accept, and where the behavior went. A removed key's own name is the
/// least useful thing to say about it: the author asked for a behavior, and needs its successor.
const REMOVED_OPTS: &[(&str, &str)] = &[(
    "spec",
    "was removed in prova 0.18 (gone, not bridged) — an OPEN proof is flagged \
     `promises = \"why it is open\"`, and the obligation it discharges is addressed by \
     `covers = \"docs/design/x.md#claim-id\"`",
)];

/// Refuse an option prova cannot honor
/// (docs/design/agent-ergonomics.md#unknown-test-opts-silently-ignored).
///
/// A dropped option is worse than a rejected one, because it reads as *configured*:
/// `tiemout = "10m"` means unbounded, and the suite that believes it is bounded finds out from a
/// hung CI job. The removed-spelling case is the same failure with a receipt — when `spec = { … }`
/// stopped being read, every suite still carrying it had its TOLERATED open specs quietly become
/// hard failures.
///
/// Unknown keys are collected and sorted before reporting: Lua table order is unspecified, and a
/// diagnostic that names a different key on each run is not a diagnostic.
pub(super) fn reject_unknown_opts(
    t: &mlua::Table,
    accepted: &[&str],
    site: &str,
) -> mlua::Result<()> {
    let mut unknown: Vec<String> = Vec::new();
    let mut positional = 0usize;
    for pair in t.clone().pairs::<Value, Value>() {
        let (k, _) = pair?;
        match k {
            Value::String(s) => {
                let key = s.to_string_lossy();
                if !accepted.contains(&key.as_ref()) {
                    unknown.push(key.to_string());
                }
            }
            // A positional entry is the same silent drop wearing a different shape:
            // `{ "slow" }` looks like tags to the author and is nothing to prova.
            _ => positional += 1,
        }
    }
    if unknown.is_empty() && positional == 0 {
        return Ok(());
    }
    unknown.sort();
    let advertised: Vec<&str> = accepted
        .iter()
        .copied()
        .filter(|k| !REMOVED_OPTS.iter().any(|(r, _)| r == k) && *k != "resources")
        .collect();
    let mut parts: Vec<String> = unknown
        .iter()
        .map(|key| match REMOVED_OPTS.iter().find(|(r, _)| r == key) {
            Some((_, teaching)) => format!("`{key}` {teaching}"),
            None => match crate::suggest::nearest(key, advertised.iter().copied()) {
                Some(best) => format!("unknown option `{key}` — did you mean `{best}`?"),
                None => format!("unknown option `{key}`"),
            },
        })
        .collect();
    if positional > 0 {
        parts.push(format!(
            "{positional} positional entr{} in the opts table — options are named \
             (`tags = {{ \"slow\" }}`, not `{{ \"slow\" }}`)",
            if positional == 1 { "y" } else { "ies" }
        ));
    }
    Err(mlua::Error::RuntimeError(format!(
        "{site}: {} (accepted: {}). An option prova cannot honor is refused, never dropped — a \
         dropped one reads as configured.",
        parts.join("; "),
        advertised.join(", ")
    )))
}

/// The `falsified_by` opt: a **function** that mutates the system so the body must go red.
///
/// A proof that has only ever been green is not evidence — it might be checking the contract, or
/// it might be checking nothing, and the two are indistinguishable in a report. Declaring the
/// mutation makes the negative case a checkable artifact instead of something a careful author
/// once did by hand and nobody repeated.
///
/// Rejected loudly when misdeclared: a falsifier that is quietly ignored is worse than none,
/// because the suite then claims a rigor it does not have.
pub(super) fn parse_falsified_by(v: &Value) -> mlua::Result<Option<Function>> {
    match v {
        Value::Nil => Ok(None),
        Value::Function(f) => Ok(Some(f.clone())),
        _ => Err(mlua::Error::RuntimeError(
            "falsified_by takes a function that breaks the system so the body fails — \
             `falsified_by = function(t) … end`; remove the entry if there is nothing to break"
                .into(),
        )),
    }
}

/// `falsified_by` is test-level, like `spec` and `proves`. A group or flow cannot carry one: the
/// mutation has to be paired with the assertion it must break, and a container has no assertions
/// of its own to invalidate.
pub(super) fn reject_falsifier(falsifier: Option<Function>, what: &str) -> mlua::Result<()> {
    if falsifier.is_some() {
        return Err(mlua::Error::RuntimeError(format!(
            "falsified_by is test-level — a {what} has no assertion of its own for a mutation to \
             break; move it onto the test whose body must go red"
        )));
    }
    Ok(())
}

pub(super) fn parse_opts(t: &mlua::Table) -> mlua::Result<UnitOpts> {
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
    let lock_vals = match (
        t.get::<Option<Vec<Value>>>("locks")?,
        t.get::<Option<Vec<Value>>>("resources")?,
    ) {
        (Some(_), Some(_)) => {
            return Err(mlua::Error::RuntimeError(
                "a unit carries `locks` or the deprecated `resources`, not both — they are one \
                 option; keep `locks`"
                    .into(),
            ))
        }
        (Some(vals), None) => Some(vals),
        (None, Some(vals)) => {
            // The pre-rename spelling: the scheduler's tokens were never the topology world's
            // provisioned resources, and one word for both taught the wrong model. Warn once
            // per load, keep working until 1.0.
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                eprintln!(
                    "prova: `resources = {{ … }}` is deprecated — the option is `locks` (same \
                     grammar: prova.writes/reads/port; retires at 1.0)"
                );
            });
            Some(vals)
        }
        (None, None) => None,
    };
    let locks = match lock_vals {
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
    let switch = match t.get::<Option<String>>("switch")? {
        Some(s) if s.trim().is_empty() => {
            return Err(mlua::Error::RuntimeError(
                "switch takes the opt-in class's name — `switch = \"ut\"` (off unless thrown \
                 with -s or a profile's switches)"
                    .into(),
            ))
        }
        other => other,
    };
    let covers = parse_covers_opt(&t.get::<Value>("covers")?)?;
    let promises = parse_promises_opt(&t.get::<Value>("promises")?)?;
    let proves = parse_proves_opt(&t.get::<Value>("proves")?)?;
    if promises.is_some() && proves.is_some() {
        return Err(mlua::Error::RuntimeError(
            "a test carries promises or proves, not both — while the work is open its context lives in the promise's reason; change the flag to proves when the promise is kept".into(),
        ));
    }
    Ok(UnitOpts {
        timeout,
        tags,
        depends_on,
        locks,
        serial,
        requires,
        switch,
        promises,
        proves,
        covers,
    })
}

/// The `covers` opt: one address, or a list of them — `"docs/design.md#claim-id"` or a ticket.
/// A proof may discharge several obligations, and an obligation may need several proofs, so this
/// is many-to-many by construction.
pub(super) fn parse_covers_opt(v: &Value) -> mlua::Result<Vec<String>> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::String(s) if !s.to_string_lossy().is_empty() => {
            Ok(vec![s.to_string_lossy().to_string()])
        }
        Value::Table(t) => {
            let addrs: Vec<String> = t.clone().sequence_values::<String>().collect::<mlua::Result<_>>()?;
            if addrs.iter().any(|a| a.is_empty()) {
                return Err(mlua::Error::RuntimeError(
                    "covers entries must be non-empty addresses".into(),
                ));
            }
            Ok(addrs)
        }
        _ => Err(mlua::Error::RuntimeError(
            "covers names the obligation(s) this proof discharges — a claim anchor \
             (\"docs/design.md#claim-id\") or a ticket, as a string or a list"
                .into(),
        )),
    }
}

/// The `promises` opt: a **non-empty reason string** — the why/ticket behind the still-open
/// contract, forced from day one (a bare `promises = true` tells the burndown nothing, and the
/// reason is what graduates into the `proves` context). There is deliberately no
/// `promises = false` — a test without the flag is already a full proof — so every wrong shape
/// is rejected with the fix, not silently accepted.
pub(super) fn parse_promises_opt(v: &Value) -> mlua::Result<Option<String>> {
    match v {
        Value::Nil => Ok(None),
        Value::String(s) if !s.to_string_lossy().is_empty() => {
            Ok(Some(s.to_string_lossy().to_string()))
        }
        Value::Boolean(false) => Err(mlua::Error::RuntimeError(
            "promises = false is not a thing — a test without the flag is already a full proof; remove the entry".into(),
        )),
        _ => Err(mlua::Error::RuntimeError(
            "promises carries the reason a contract is still open — give it a non-empty string (the why/ticket), or remove the entry".into(),
        )),
    }
}

/// The `proves` opt: graduated context — the why behind a finished proof, living in the test
/// itself. The context IS the point, so a bare `proves = true` or an empty string is refused
/// with the fix rather than accepted as a say-nothing annotation.
pub(super) fn parse_proves_opt(v: &Value) -> mlua::Result<Option<String>> {
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
pub(super) fn parse_resource(v: Value) -> mlua::Result<ResourceReq> {
    match v {
        Value::String(s) => Ok(ResourceReq {
            token: s.to_string_lossy().to_string(),
            shared: false,
            machine: false,
        }),
        Value::UserData(ud) => ud
            .borrow::<ResourceRef>()
            .map(|r| ResourceReq {
                token: r.token.clone(),
                shared: r.shared,
                machine: r.machine,
            })
            .map_err(|_| mlua::Error::RuntimeError(RESOURCE_ENTRY_ERR.into())),
        _ => Err(mlua::Error::RuntimeError(RESOURCE_ENTRY_ERR.into())),
    }
}

/// What a `locks` list accepts, said once so the two rejection paths can't drift.
pub(super) const RESOURCE_ENTRY_ERR: &str =
    "locks entries must be strings or prova.port/writes/reads refs";

/// Read the optional second argument of `prova.writes`/`reads`: `{ scope = "machine" }` widens
/// the hold to the whole box; the default (and explicit `"package"`) binds every prova instance
/// at this home. Anything else is a taught error.
pub(super) fn parse_lock_scope(opts: &Option<Table>) -> mlua::Result<Option<bool>> {
    let Some(t) = opts else { return Ok(None) };
    match t.get::<Option<String>>("scope")?.as_deref() {
        None => Ok(None),
        Some("machine") => Ok(Some(true)),
        Some("package") => Ok(Some(false)),
        Some(other) => Err(mlua::Error::RuntimeError(format!(
            "scope must be \"package\" (default — every prova at this home) or \"machine\" \
             (the whole box), got {other:?}"
        ))),
    }
}

/// Build a typed lock ref in `shared` mode from a bare token or an existing ref. Re-moding is
/// deliberate: `prova.reads(prova.port(5432))` widens a port to a concurrent hold. `machine`
/// overrides the scope when given, else the existing ref's scope carries through.
pub(super) fn resource_ref(
    lua: &Lua,
    v: Value,
    shared: bool,
    machine: Option<bool>,
) -> mlua::Result<mlua::AnyUserData> {
    let req = parse_resource(v)?;
    lua.create_userdata(ResourceRef {
        token: req.token,
        shared,
        machine: machine.unwrap_or(req.machine),
    })
}
