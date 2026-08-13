//! The context (`t` / `ctx`) — one type for test bodies and fixture factories:
//! `t:use`, `t:defer`, `t:manage`, snapshots, and the expect entry point.

use super::*;

const SKIP_SENTINEL: &str = "__prova_skip__";

// ---------------------------------------------------------------------------------------------
// The context (`t` / `ctx`) — one type for test bodies and fixture factories
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub(super) struct TestRun {
    pub(super) assertions: usize,
    pub(super) failure: Option<String>,
    pub(super) skip: Option<String>,
    /// Inside `t:expect_all(...)`, a failed assertion is collected here instead of aborting, so the
    /// block reports *every* failure. `soft` is the active flag; `soft_failures` accumulates.
    pub(super) soft: bool,
    pub(super) soft_failures: Vec<String>,
    /// Snapshot context for `matches_snapshot` (where `.snap` files live, the key base, update mode,
    /// and a per-test counter for auto-named snapshots). `None` when the test has no source file path.
    pub(super) snapshot: Option<SnapshotCtx>,
}

/// Per-test snapshot state: everything `matches_snapshot` needs to locate and key a `.snap` file.
pub(super) struct SnapshotCtx {
    /// `<test-file-dir>/snapshots`.
    pub(super) dir: PathBuf,
    /// The test-file stem — the `.snap` filename prefix (`<stem>__<key>.snap`).
    pub(super) stem: String,
    /// A slug of the test's node path — the base for auto-named snapshots (`<slug>-<n>`).
    pub(super) key_base: String,
    /// `--update-snapshots`: write instead of compare.
    pub(super) update: bool,
    /// Increments per *unnamed* `matches_snapshot` in this test, so several are distinct.
    pub(super) counter: usize,
    /// Shared registry to record each referenced `.snap` into (for unreferenced reconciliation).
    pub(super) registry: Option<SnapshotRegistry>,
}

/// Injected into every body/factory. `own_scope` is the scope its `defer`/`tempdir` target and the
/// floor for the scope-mismatch check; `test_scope` is the active test/step scope instance;
/// `flow_scope` is the enclosing flow's scope instance (present only inside a flow).
///
/// `Clone` is cheap (all fields are `Rc`/`Copy`) and lets the async `use` method own a snapshot in
/// its future without holding the userdata borrow across an `await`.
#[derive(Clone)]
pub(super) struct Ctx {
    pub(super) run: Rc<RefCell<TestRun>>,
    pub(super) state: Rc<RunState>,
    pub(super) test_scope: Rc<RefCell<ScopeState>>,
    /// This test's file scope instance (`Scope.File`) — its file's, so it is shared across the file's
    /// tests but distinct per file within a suite.
    pub(super) file_scope: Rc<RefCell<ScopeState>>,
    pub(super) flow_scope: Option<Rc<RefCell<ScopeState>>>,
    pub(super) own_scope: ScopeKind,
    /// The `test_each` case for this test, exposed as `t.case` (also passed as the body's 2nd arg).
    /// `None` (→ `nil`) for ordinary tests and for fixture factory contexts.
    pub(super) case: Option<Value>,
    /// True only for the context injected into a `prova.topology` factory: it makes `ctx.network`
    /// return the topology's ambient managed network (lazily created + scope-managed). Every other
    /// context — test bodies, ordinary `prova.fixture` factories, `prova eval` — leaves it `false`,
    /// so `ctx.network` is nil and resources provisioned there never auto-join a network.
    pub(super) topology: bool,
}

impl Ctx {
    pub(super) fn scope_state(&self, kind: ScopeKind) -> mlua::Result<Rc<RefCell<ScopeState>>> {
        Ok(match kind {
            // A run-scoped fixture crosses Lua states, and a ScopeState (teardowns, tempdirs)
            // belongs to one: a defer registered here could not run after its state is gone. A
            // conduct's artifact lives in the tree (e.g. target/), reaped by its next conduct.
            ScopeKind::Run => {
                return Err(mlua::Error::RuntimeError(
                    "a run-scoped fixture has no ctx:defer/ctx:tempdir — its value must be plain \
                     data and its artifacts live in the tree, because the run outlives every Lua \
                     state (docs/plans/shared-deputies.md)"
                        .into(),
                ))
            }
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
    pub(super) fn own_scope_state(&self) -> mlua::Result<Rc<RefCell<ScopeState>>> {
        self.scope_state(self.own_scope)
    }
}

/// Resolve `ctx:use(handle|name)` to a fixture value, building it lazily if not cached. Async so a
/// factory can `await` (e.g. `shell.run`, `http.wait_for`). Recursion (a factory that itself calls
/// `ctx:use`) reenters through Lua, not Rust, so no boxing is needed. No `RefCell` borrow is held
/// across the `await`.
pub(super) async fn resolve_use(lua: &Lua, this: &Ctx, target: Value) -> mlua::Result<Value> {
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

    // Run scope resolves through the run-wide store, not a state's ScopeState — the instance
    // must cross Lua states and workers (docs/plans/shared-deputies.md).
    if def.scope == ScopeKind::Run {
        return resolve_run_scoped(lua, this, &def).await;
    }

    let ss = this.scope_state(def.scope)?;
    if let Some(v) = ss.borrow().cache.get(&id) {
        return Ok(v.clone());
    }
    // Failure memoizes exactly like success (the poison lives and dies with this scope
    // instance): the recorded error replays to every later consumer, named as a replay so a
    // memoized verdict can never read as a fresh attempt.
    if let Some(err) = ss.borrow().poisoned.get(&id) {
        return Err(mlua::Error::RuntimeError(format!(
            "fixture {:?} already failed in this {} scope — memoized, not re-provisioned: {err}",
            def.name,
            def.scope.label()
        )));
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
    let value: Value = match def.factory.call_async(child_ud).await {
        Ok(v) => v,
        Err(e) => {
            ss.borrow_mut().poisoned.insert(id, e.to_string());
            return Err(e);
        }
    };
    ss.borrow_mut().cache.insert(id, value.clone());
    Ok(value)
}

/// Resolve a `Scope.Run` fixture through the run-wide conduct store: lazy, blocking,
/// single-flight (docs/plans/shared-deputies.md). Whichever consumer asks first conducts;
/// everyone else — same suite or another worker — waits for the settled slot, and waiting IS the
/// ordering. Values are data: serialized once by the conducting state, deserialized per reader
/// (the conductor included, so every consumer holds an identical copy and nothing Lua crosses a
/// state boundary). Failure settles the slot exactly as success does — the run-instance form of
/// docs/design/lifecycle.md#fixture-failure-memoization.
async fn resolve_run_scoped(lua: &Lua, this: &Ctx, def: &FixtureDef) -> mlua::Result<Value> {
    use crate::engine::ConductSlot;
    let store = this.state.conducts.clone();
    // Set on the first poll that finds another worker conducting; dropping it reports how long
    // this reader waited, on every path out of the loop.
    let mut waiting: Option<crate::progress::Activity> = None;
    loop {
        {
            let mut slots = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match slots.get(&def.name) {
                None => {
                    slots.insert(def.name.clone(), ConductSlot::Conducting);
                    break;
                }
                Some(ConductSlot::Ready(v)) => {
                    let v = v.clone();
                    drop(slots);
                    return crate::modules::formats::json_value_to_lua(lua, &v);
                }
                Some(ConductSlot::Poisoned(err)) => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "fixture {:?} already failed in this run — memoized, not re-provisioned: {err}",
                        def.name
                    )));
                }
                Some(ConductSlot::Conducting) => {}
            }
        }
        // Say what is being waited FOR (docs/design/agent-ergonomics.md#narrate-lock-waits): this
        // wait lands inside the READER's own duration, so without a word here a queued reader is
        // indistinguishable from a slow one — the shape behind a unit that reads 848.8s for 190s
        // of work. Started on the first poll, so the threshold drops a conduct that settles fast;
        // dropping it at every exit reports the duration whichever way the wait ends.
        if waiting.is_none() {
            waiting = Some(crate::progress::start(
                &this.state.progress,
                crate::progress::Kind::Waiting,
                format!("{:?} — conducted by another worker in this run", def.name),
            ));
        }
        // Async sleep, never a thread block: a same-state waiter must not wedge the thread that
        // is driving the very conduct it waits on.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // This consumer claimed the slot; it settles whatever happens — a cancelled or panicked
    // conduct must poison, not strand every waiter on `Conducting` forever.
    struct Settle {
        store: crate::engine::ConductRegistry,
        name: String,
        done: bool,
    }
    impl Drop for Settle {
        fn drop(&mut self) {
            if !self.done {
                self.store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        self.name.clone(),
                        crate::engine::ConductSlot::Poisoned(
                            "the conduct was abandoned before settling (cancelled or panicked)"
                                .into(),
                        ),
                    );
            }
        }
    }
    let mut settle = Settle {
        store: store.clone(),
        name: def.name.clone(),
        done: false,
    };

    let child = Ctx {
        run: this.run.clone(),
        state: this.state.clone(),
        test_scope: this.test_scope.clone(),
        file_scope: this.file_scope.clone(),
        flow_scope: this.flow_scope.clone(),
        own_scope: ScopeKind::Run,
        case: None,
        topology: def.is_topology,
    };
    let child_ud = lua.create_userdata(child)?;
    let settled = match def.factory.call_async::<Value>(child_ud).await {
        Ok(value) => match crate::modules::formats::lua_value_to_json(lua, &value) {
            Ok(json) => Ok(json),
            Err(e) => Err(format!(
                "a run-scoped fixture's value must be plain data (JSON-serializable) — it \
                 crosses Lua states, so functions and userdata cannot travel: {e}"
            )),
        },
        Err(e) => Err(e.to_string()),
    };
    let result = {
        let mut slots = store.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match settled {
            Ok(json) => {
                let out = crate::modules::formats::json_value_to_lua(lua, &json);
                slots.insert(def.name.clone(), ConductSlot::Ready(json));
                out
            }
            Err(msg) => {
                slots.insert(def.name.clone(), ConductSlot::Poisoned(msg.clone()));
                Err(mlua::Error::RuntimeError(msg))
            }
        }
    };
    settle.done = true;
    result
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
            let s = crate::modules::emit_path(&path);
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
