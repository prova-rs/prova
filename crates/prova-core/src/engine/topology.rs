//! `prova up` and warm topology holding: provision a named topology, report its
//! endpoints, and hold it live for runs/evals until shutdown.

use super::*;

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
    on_ready: impl FnOnce(&[Endpoint], &serde_json::Value),
) -> mlua::Result<()> {
    let (lua, _col, state, id) = load_topology(files, name, config)?;

    let rt = new_runtime()?;
    block_on_local(&rt, async {
        // The signal is raced against provisioning, not awaited after it
        // (docs/design/agent-ergonomics.md#start-timeout-orphans-containers). A stack slow enough to
        // be interrupted is interrupted DURING provisioning — that is when a budget expires — and
        // until this select existed, SIGTERM then found no handler installed yet, so the holder died
        // by default disposition and every resource it had already created outlived it. Cancelling
        // the provision future drops it mid-flight; whatever it registered is already held by the
        // File scope below, which is what teardown reaps.
        let result = tokio::select! {
            r = provision_and_hold(&lua, &state, id, name, on_ready) => r,
            () = wait_for_shutdown() => Ok(()),
        };
        // Always tear down whatever got provisioned — a clean signal, a mid-provision failure, or
        // an interrupt part-way through.
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
pub(super) fn exec_topology_registrations(lua: &Lua, config: &RunConfig) -> mlua::Result<()> {
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
                "invalid [topologies] entry {:?}: name must be [A-Za-z0-9_-]+, and package/factory \
                 dotted identifier paths (got package={:?}, factory={:?})",
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

/// Register everything the topology verbs address: exec the files (declarations only — no
/// factory runs, so no docker) plus the manifest's `[topologies]` registrations.
fn load_topology_state(files: &[PathBuf], config: &RunConfig) -> mlua::Result<(Lua, SharedCollector)> {
    let (lua, col) = build_lua("up".to_string(), config)?;
    for file in files {
        let code = std::fs::read_to_string(file).map_err(|e| {
            mlua::Error::RuntimeError(format!("cannot read {}: {e}", file.display()))
        })?;
        lua.load(&code).set_name(file_chunk_name(file)).exec()?;
    }
    exec_topology_registrations(&lua, config)?;
    Ok((lua, col))
}

/// Enumerate the topology names available — every `prova.topology(name, fn)` the `files` declare,
/// plus every `[topologies]` registration — sorted. Only *registers* them (execs the files); it never
/// invokes a factory, so it needs no docker. The discovery half of `up` (`prova up` with no name).
pub fn list_topologies(files: &[PathBuf], config: &RunConfig) -> mlua::Result<Vec<String>> {
    let (_lua, col) = load_topology_state(files, config)?;
    let names: Vec<String> = col.borrow().topologies.keys().cloned().collect();
    Ok(names)
}

pub(super) fn load_topology(
    files: &[PathBuf],
    name: &str,
    config: &RunConfig,
) -> mlua::Result<(Lua, SharedCollector, Rc<RunState>, usize)> {
    let (lua, col) = load_topology_state(files, config)?;

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
        falsify: false,
        conducts: config.conducts.clone(),
        progress: std::sync::Arc::clone(config.progress()),
        project_dir: config.project_dir.clone(),
    });
    Ok((lua, col, state, id))
}

/// Instantiate the topology under a held `Scope.File`, report its endpoints, and block until a
/// shutdown signal. Separated so `up` can run teardown unconditionally afterward — even if the factory
/// raises mid-provision, the File scope already holds teardowns for whatever came up.
pub(super) async fn provision_and_hold(
    lua: &Lua,
    state: &Rc<RunState>,
    id: usize,
    topo_name: &str,
    on_ready: impl FnOnce(&[Endpoint], &serde_json::Value),
) -> mlua::Result<()> {
    let (value, endpoints) = provision(lua, state, id, topo_name).await?;
    // The holder's record carries a JSON projection of the factory's returned value — the
    // rehydration payload an attaching run seeds instead of provisioning (see `json_to_lua`).
    let snapshot = eval_value_to_json(lua, &value, 0);
    on_ready(&endpoints, &snapshot);
    wait_for_shutdown().await;
    Ok(())
}

/// Instantiate the topology under a held `Scope.File` and return its live value plus its endpoints.
/// The provisioned resources stay alive via the File scope's teardowns (held in `state`) until the
/// caller reaps them; separated from the wait/hold so `up` (hold until signal), `watch` (hold until
/// change), and `hold_topology` (hold across MCP tool calls) all reuse it.
pub(super) async fn provision(
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

/// Rehydrate a recorded JSON value into a Lua value — the inverse of the projection a holder
/// records (`eval_value_to_json`). Attach seeds the result into scope caches, so what a test's
/// `t:use(<topology>)` sees is exactly the JSON-representable structure the holder's factory
/// returned: urls, hosts, ports, network vantages, plain data. Closures and userdata did not
/// survive the projection — by design, the grammar's answer is "clients attach by url".
pub(super) fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        J::String(s) => Value::String(lua.create_string(s.as_str())?),
        J::Array(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        J::Object(fields) => {
            let t = lua.create_table()?;
            for (k, val) in fields {
                t.set(k.as_str(), json_to_lua(lua, val)?)?;
            }
            Value::Table(t)
        }
    })
}

/// Walk a topology's returned value for connect strings. Each field whose value is a table with a
/// string `url` becomes an endpoint (`db → postgres://…`); a top-level `url` (a single-resource
/// topology) is reported under the topology's own name.
pub(super) fn extract_endpoints(value: &Value, topo_name: &str) -> Vec<Endpoint> {
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

        // Re-exec the `[topologies]` registrations the config carries, for names the files did
        // NOT declare — a registered-only topology (no code declaration anywhere) stays
        // warm-runnable after the collector reset. Names the files DID declare are left alone:
        // that is the both-doors package, and `prova.topology` refuses duplicates by design.
        {
            let declared: std::collections::HashSet<String> =
                self.col.borrow().topologies.keys().cloned().collect();
            let mut cfg = self.config.clone();
            cfg.topology_registrations
                .retain(|r| !declared.contains(&r.alias));
            exec_topology_registrations(&self.lua, &cfg)?;
        }

        let (plan, deselected, dropped, switched_off, state) = {
            let col = self.col.borrow();
            let plan = build_plan(&col, &self.config.capabilities)?;
            // Same order as the cold path: held-back classes leave the membership before selection.
            let (plan, switch_deselected, switch_dropped, switched_off) =
                apply_switch_filter(plan, &self.config.switches, selection);
            let (plan, mut deselected, mut dropped) = apply_selection(plan, selection);
            let (plan, falsify_deselected, falsify_dropped) =
                apply_falsify_filter(plan, self.config.falsify);
            let (plan, spec_deselected, spec_dropped) =
                apply_specs_filter(plan, self.config.promises_only, self.config.proofs_only);
            deselected += switch_deselected + falsify_deselected + spec_deselected;
            dropped.extend(switch_dropped);
            dropped.extend(falsify_dropped);
            dropped.extend(spec_dropped);
            let dropped = qualify_all(dropped, &col.file_paths);

            // A fresh run state — the run's own scopes, so its teardown reaps only what it built.
            let state = Rc::new(RunState {
                defs: col.fixtures.clone(),
                suite: Rc::new(RefCell::new(ScopeState::default())),
                files: RefCell::new(HashMap::new()),
                file_paths: col.file_paths.clone(),
                update_snapshots: self.config.update_snapshots,
                snapshot_registry: self.config.snapshot_registry.clone(),
                falsify: self.config.falsify,
                conducts: self.config.conducts.clone(),
                progress: std::sync::Arc::clone(self.config.progress()),
                project_dir: self.config.project_dir.clone(),
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
            (plan, deselected, dropped, switched_off, state)
        };

        let mut config = self.config.clone();
        config.selection = selection.clone();

        reporter.event(&Event::RunStarted);
        let mut summary = Summary {
            deselected,
            deselected_paths: dropped,
            switched_off,
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
            falsify: false,
            conducts: self.config.conducts.clone(),
            progress: std::sync::Arc::clone(self.config.progress()),
            project_dir: self.config.project_dir.clone(),
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
pub(super) async fn wait_for_shutdown() {
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
pub(super) async fn wait_for_change_or_shutdown(files: &[PathBuf]) -> bool {
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
pub(super) fn snapshot_mtimes(files: &[PathBuf]) -> Vec<Option<std::time::SystemTime>> {
    files
        .iter()
        .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The held-topology snapshot bridge: a run-state JSON round-trips into the Lua shapes an
    /// attaching run's fixtures expect — nested tables, 1-based arrays, nil for null. What a
    /// warm attach actually hands a proof.
    #[test]
    fn json_to_lua_round_trips_the_snapshot_shapes() {
        let lua = Lua::new();
        let snapshot: serde_json::Value = serde_json::json!({
            "db": { "url": "postgres://u", "port": 5432, "ready": true },
            "aliases": ["a", "b"],
            "gone": null,
        });
        let v = json_to_lua(&lua, &snapshot).unwrap();
        let Value::Table(t) = v else { panic!("expected a table") };
        let db: Table = t.get("db").unwrap();
        assert_eq!(db.get::<String>("url").unwrap(), "postgres://u");
        assert_eq!(db.get::<f64>("port").unwrap(), 5432.0);
        assert!(db.get::<bool>("ready").unwrap());
        let aliases: Table = t.get("aliases").unwrap();
        assert_eq!(aliases.get::<String>(1).unwrap(), "a");
        assert_eq!(aliases.get::<String>(2).unwrap(), "b");
        assert!(matches!(t.get::<Value>("gone").unwrap(), Value::Nil));
    }
}
