//! `prova eval` — a one-shot snippet evaluated in the full proof environment.

use super::*;

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

    // An ASYNC function, and `call_async` below, because a predicate is where you probe a real
    // dependency — and an async-backed probe (`http`, `grpc`, `docker`) can only yield from a
    // coroutine. A sync registrar calling a sync `f.call(())` leaves the predicate no way to await,
    // which surfaces as "attempt to yield from outside a coroutine".
    let registrar = lua
        .create_async_function(move |_, (name, f): (String, mlua::Function)| {
            let caps_w = caps_w.clone();
            async move {
            if is_builtin_capability(&name) {
                return Err(mlua::Error::RuntimeError(format!(
                    "runtime.capability({name:?}): {name:?} is a built-in capability and cannot be \
                     redefined — `requires = {{ {name:?} }}` must mean the same thing in every project"
                )));
            }
            // The predicate runs NOW, at load; only its answer survives (see `Capabilities`).
            // Recorded through the same function the manifest path uses, so a predicate migrated
            // from here to a package keeps its body AND its meaning.
            let verdict: Value = f.call_async(()).await?;
            record_verdict(&mut caps_w.borrow_mut(), &name, verdict)
                .map_err(mlua::Error::RuntimeError)?;
            Ok(())
            }
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

    // Inside a runtime, and as a coroutine (`exec_async`), because a capability predicate is exactly
    // where you probe a real dependency — `runtime.capability`'s own docs offer "a GPU, a licence
    // file, a kind cluster", and two of those three want to make a call.
    //
    // The companion used to load from plain sync `main()`, so any async-backed API panicked with
    // "there is no reactor running". Supplying a reactor alone was not enough: a sync `exec` gives
    // the chunk no coroutine to yield from. Going through `block_on_local` (as every other execution
    // path does) supplies the reactor and keeps `spawn_local` working; `exec_async` supplies the
    // coroutine.
    //
    // Found by a plugin whose predicate validated a registry credential over HTTP: the panic was
    // swallowed into "unreachable", the gate degraded to a presence-only check, and the suite ran on
    // a credential the registry rejects — the vacuous green this function's own doc-comment warns
    // about, arriving by a route it did not anticipate.
    let rt = new_runtime().map_err(|e| format!("{}: {e}", path.display()))?;
    block_on_local(&rt, async {
        lua.load(&src)
            .set_name(file_chunk_name(path))
            .exec_async()
            .await
            .map_err(|e| format!("{}: {e}", path.display()))
    })?;

    let out = caps.borrow().clone();
    Ok(out)
}

/// Resolve the manifest's `[capabilities]` declarations into the run's vocabulary
/// (docs/design/capabilities.md).
///
/// The declarative kinds (`command`, `intrinsic`) are pure data: validated here, probed later, on
/// first reference. Only the `package` kind needs Lua — and only if at least one entry uses it,
/// which is why the state is built lazily. A project with no Lua predicates pays nothing: no state,
/// no runtime, no package searcher.
///
/// A declaration that cannot be resolved is an **error**, never a warning. Every capability it meant
/// to declare would otherwise go silently missing, so every gated test would skip and the run would
/// be green — the vacuous green, one level out from the suite.
pub fn resolve_capabilities(
    registrations: &[CapabilityRegistration],
    policy: UndeclaredPolicy,
    config: &RunConfig,
) -> Result<Capabilities, String> {
    let mut caps = Capabilities::default();
    caps.set_undeclared_policy(policy);

    // Pass one: the data-only kinds, and validation for everything.
    let mut lua_needed: Vec<&CapabilityRegistration> = Vec::new();
    for reg in registrations {
        if !is_capability_name(&reg.name) {
            return Err(format!(
                "[capabilities] {:?}: a capability name must be [A-Za-z0-9_-]+ (the one other key \
                 is \"*\", the fall-through policy)",
                reg.name
            ));
        }
        match &reg.factory {
            CapabilityFactory::Command(probe) => {
                probe.validate(&reg.name)?;
                caps.declare_command(&reg.name, probe.clone());
            }
            CapabilityFactory::Intrinsic(preset) => {
                // A preset that is not a built-in is a typo, and a typo that resolved to "absent"
                // would skip every gated test forever while reading as a deliberate declaration.
                if !is_builtin_capability(preset) {
                    return Err(format!(
                        "[capabilities] {}: intrinsic = {preset:?} is not one of prova's built-in \
                         checkers — they are: {}",
                        reg.name,
                        builtin_capability_names().join(", ")
                    ));
                }
                caps.declare_intrinsic(&reg.name, preset);
            }
            CapabilityFactory::Package { package, factory, .. } => {
                if !is_ident_path(package) || !is_ident_path(factory) {
                    return Err(format!(
                        "[capabilities] {}: package and factory must be dotted identifier paths \
                         (got package={package:?}, factory={factory:?})",
                        reg.name
                    ));
                }
                lua_needed.push(reg);
            }
        }
    }
    if lua_needed.is_empty() {
        return Ok(caps);
    }

    // Pass two: the Lua predicates. One state for all of them — they are a project's vocabulary, so
    // they share a state the way the companion's registrations did.
    let (lua, _col) = build_lua("capabilities".to_string(), config)
        .map_err(|e| format!("resolving [capabilities]: {e}"))?;
    // Inside a runtime, and as a coroutine, because a predicate is exactly where you probe a real
    // dependency (`http`, `grpc`, `docker`) — and an async-backed probe can only yield from one. A
    // sync call leaves the predicate no way to await, which surfaces as the baffling "attempt to
    // yield from outside a coroutine".
    let rt = new_runtime().map_err(|e| format!("resolving [capabilities]: {e}"))?;
    block_on_local(&rt, async {
        for reg in &lua_needed {
            let CapabilityFactory::Package {
                package,
                factory,
                options,
            } = &reg.factory
            else {
                continue;
            };
            let call = match options {
                None => format!("return (require(\"{package}\")).{factory}()"),
                Some(opts) => format!("return (require(\"{package}\")).{factory}({opts})"),
            };
            let verdict: Value = lua
                .load(&call)
                .set_name(format!("@[capabilities].{}", reg.name))
                .eval_async()
                .await
                .map_err(|e| {
                    format!(
                        "capability {:?} (require(\"{package}\").{factory}): {e}",
                        reg.name
                    )
                })?;
            record_verdict(&mut caps, &reg.name, verdict)?;
        }
        Ok::<(), String>(())
    })?;

    Ok(caps)
}

/// Store what a Lua predicate answered. The contract is the same one the companion's registrar had,
/// which is what lets a migrated predicate keep its body unchanged:
///   - `true`            → available, no version
///   - a version string  → available, and comparable (`requires = { "gpu >= 2.0" }`)
///   - `false` / `nil`   → unavailable
///
/// Anything else is an error rather than a coerced truthy value: Lua's truthiness would make a typo'd
/// return (a table, a number) read as "available", which is the direction that produces a false green.
fn record_verdict(caps: &mut Capabilities, name: &str, verdict: Value) -> Result<(), String> {
    match verdict {
        // Recorded as a DECLARED no, so it never falls through to a PATH probe that could answer yes
        // about an unrelated binary of the same name
        // (docs/design/capabilities.md#a-declared-no-is-final).
        Value::Nil | Value::Boolean(false) => caps.register_absent(name),
        Value::Boolean(true) => caps.register(name, None),
        Value::String(s) => {
            let raw = s.to_str().map_err(|e| e.to_string())?.to_string();
            let v = parse_first_version(&raw).ok_or_else(|| {
                format!(
                    "capability {name:?}: the predicate returned {raw:?}, which is not a version \
                     (expected true/false, or a version string like \"2.4.0\")"
                )
            })?;
            caps.register(name, Some(v));
        }
        other => {
            return Err(format!(
                "capability {name:?}: the predicate returned {}, expected a boolean or a version \
                 string",
                other.type_name()
            ))
        }
    }
    Ok(())
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
        falsify: false,
        conducts: config.conducts.clone(),
        progress: std::sync::Arc::clone(config.progress()),
        project_dir: config.project_dir.clone(),
        // An `eval` provisions for itself: a snippet is not a run, and routing it through a run's
        // topology pool would hand it an instance nothing here reaps.
        interned: None,
    });

    let rt = new_runtime()?;
    eval_with_state(&lua, &rt, code, &state)
}

/// The shared eval executor: compile `code`, expose a transient `ctx` over `state`'s File scope,
/// run it inside `rt`, tear the transient scope down (success OR error), and JSON-ify the value.
/// Used by the one-shot `eval_snippet` (fresh Lua/runtime) and by `HeldTopology::eval_warm` (the
/// holder's Lua/runtime, with the held instance pre-seeded into `state`).
pub(super) fn eval_with_state(
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
pub(super) fn eval_value_to_json(lua: &Lua, v: &Value, depth: usize) -> serde_json::Value {
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
pub(super) fn eval_tostring(lua: &Lua, v: &Value) -> String {
    lua.globals()
        .get::<Function>("tostring")
        .and_then(|f| f.call::<String>(v.clone()))
        .unwrap_or_else(|_| format!("<{}>", v.type_name()))
}
