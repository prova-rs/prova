//! Setup: build the Lua state for a suite -- the `prova` global, the registration
//! surface (test/group/describe/flow), and the collector behind it.

use super::*;

// ---------------------------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------------------------

/// Route `os.getenv` through Rust's view of the environment.
///
/// Lua's own `os.getenv` reads the C runtime's copy of the environment. On Windows that copy is
/// a snapshot taken at startup, and `std::env::set_var` (SetEnvironmentVariableW) does not
/// update it — so a manifest's `[run.env]`, which we inject with set_var, reached spawned child
/// processes but was invisible to the tests themselves. On Unix the two views are the same table,
/// which is why only Windows saw it. Reading through Rust makes `os.getenv` agree everywhere, and
/// agree with what `shell.run` children inherit.
fn install_os_env(lua: &Lua) -> mlua::Result<()> {
        let os: mlua::Table = lua.globals().get("os")?;
        os.set(
            "getenv",
            lua.create_function(|_, name: String| Ok(std::env::var(name).ok()))?,
        )?;
    Ok(())
}

/// The registration surface: `prova.test` / `test_each` / `group` / `flow` / `describe`.
fn install_registration(lua: &Lua, prova: &Table, col: &SharedCollector) -> mlua::Result<()> {
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
    Ok(())
}

/// `prova.fixture` and `prova.topology` — typed handles into the collector's fixture registry.
fn install_fixtures(lua: &Lua, prova: &Table, col: &SharedCollector) -> mlua::Result<()> {
    {
        let col = col.clone();
        prova.set(
            "fixture",
            lua.create_function(move |lua, (name, a, b, opts): (String, Value, Value, Value)| {
                let (scope, factory) = match (a, b) {
                    (Value::Function(f), Value::Nil) => (ScopeKind::Test, f),
                    (scope_val, Value::Function(f)) => (parse_scope(scope_val)?, f),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "fixture(name, scope, factory, opts?)".into(),
                        ))
                    }
                };
                let identity = parse_identity(&name, scope, &opts)?;
                let id = {
                    let mut c = col.borrow_mut();
                    // One name, one contract — exactly the rule topologies already enforce. Two
                    // same-named declarations silently forked into two instances (two conducts of
                    // one deputy); for `Scope.Run` the name is a run-wide slot, so an ambiguous
                    // one is a defect (the duplicate-claim-id precedent). Share one declaration
                    // via require() instead.
                    if c.fixtures.iter().any(|f| f.name == name) {
                        return Err(mlua::Error::RuntimeError(format!(
                            "fixture {name:?} is already defined — a fixture name is one \
                             contract (for Scope.Run, one run-wide slot); require() the one \
                             declaration instead of redeclaring it"
                        )));
                    }
                    if scope == ScopeKind::Suite && c.singleton_suite {
                        eprintln!(
                            "prova: {}: fixture {name:?} is Scope.Suite in a file that is its \
                             own suite — no suite.lua groups this directory, so the scope \
                             behaves as Scope.File; add a suite.lua to share across the \
                             directory's files, or say Scope.File",
                            c.file_paths.first().map(|p| p.display().to_string()).unwrap_or_default()
                        );
                    }
                    let id = c.fixtures.len();
                    c.fixtures.push(FixtureDef {
                        name,
                        scope,
                        factory,
                        is_topology: false,
                        identity,
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
                        identity: None,
                    });
                    c.topologies.insert(name, id);
                    id
                };
                lua.create_userdata(FixtureHandle { id })
            })?,
        )?;
    }
    Ok(())
}

/// `prova.remind` — the attention account's declaration form.
fn install_remind(lua: &Lua, prova: &Table, col: &SharedCollector) -> mlua::Result<()> {
    {
        // prova.remind(name, { when = fn, requires? }, message) — an obligation the WORLD creates:
        // the attention account, not the evidence account (docs/design/reminders.md). Declared
        // beside tests, collected like them, and deliberately NOT one: it never enters the plan,
        // the selection, burndown, or the tally. The condition evaluates after the proofs complete
        // (see `evaluate_reminders`); the message is the instruction, because a reminder's
        // discharge is an act, not an assertion.
        let col = col.clone();
        prova.set(
            "remind",
            lua.create_function(
                move |lua, (name, opts, message): (String, Table, Option<String>)| {
                    reject_bare_in_builder(&col, "remind")?;
                    if name.trim().is_empty() {
                        return Err(mlua::Error::RuntimeError(
                            "remind needs a name — it is how the reminder reports".into(),
                        ));
                    }
                    let message = message.filter(|m| !m.trim().is_empty()).ok_or_else(|| {
                        mlua::Error::RuntimeError(
                            "remind(name, opts, message): the message is the instruction — say \
                             what to DO when this fires (a reminder is discharged by an act, so \
                             it carries a to-do, not an assertion)"
                                .into(),
                        )
                    })?;
                    let when: Function =
                        opts.get::<Option<Function>>("when")?.ok_or_else(|| {
                            mlua::Error::RuntimeError(
                                "remind needs `when = function(account) ... end` — the condition \
                                 that makes it due (return falsy for quiet, or true/a why-string \
                                 when attention is owed)"
                                    .into(),
                            )
                        })?;
                    let own_requires: Vec<String> = opts
                        .get::<Option<Vec<String>>>("requires")?
                        .unwrap_or_default();
                    let tags: Vec<String> =
                        opts.get::<Option<Vec<String>>>("tags")?.unwrap_or_default();
                    let line = caller_line(lua, &col);
                    let mut c = col.borrow_mut();
                    let file = c.current_file;
                    // Gating follows scope (docs/design/reminders.md#reminder-gating-follows-scope):
                    // a reminder inherits the ambient chain's `requires` — `suite.config`'s root
                    // fold, an enclosing describe's — exactly as a test declared here would, so
                    // on a machine missing the capability it reports `requires "…" (unavailable)`
                    // instead of whatever the condition happened to raise. Scope first, own after:
                    // the first unmet expression names the reason, and the scope's is the broader fact.
                    let mut requires: Vec<String> = Vec::new();
                    for &ix in &c.parent_stack {
                        for r in &c.nodes[ix].opts.requires {
                            if !requires.contains(r) {
                                requires.push(r.clone());
                            }
                        }
                    }
                    for r in own_requires {
                        if !requires.contains(&r) {
                            requires.push(r);
                        }
                    }
                    c.reminders.push(ReminderDef {
                        name,
                        when,
                        message,
                        requires,
                        tags,
                        file,
                        line,
                    });
                    Ok(())
                },
            )?,
        )?;
    }
    Ok(())
}

/// The async utilities: `prova.sleep` and `prova.retry`.
fn install_utilities(lua: &Lua, prova: &Table) -> mlua::Result<()> {
    prova.set(
        "sleep",
        lua.create_async_function(|_, millis: u64| async move {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            Ok(())
        })?,
    )?;

    // prova.barrier(token, parties, opts?) — block until `parties` participants have arrived, so
    // reaching the far side IS the proof they were in flight at once. The primitive concurrency
    // assertions were missing: `sleep` measures timing luck (it fails on a loaded host and PASSES
    // when a serialized system happens to overlap anyway), and the locks serialize, which is the
    // opposite. See crate::barrier.
    //
    // ASYNC, and that is load-bearing rather than stylistic. A worker drives many Lua coroutines
    // cooperatively on one current-thread runtime, so a barrier that blocked the thread would
    // starve the very sibling it waits for and deadlock itself — measured, on the first draft.
    // Awaiting between polls yields the worker, letting the other participants run and arrive.
    prova.set(
        "barrier",
        lua.create_async_function(|_, (token, parties, opts): (String, u64, Option<Table>)| async move {
            if let Some(o) = &opts {
                crate::opts::reject_unknown(o, &["timeout"], "prova.barrier")?;
            }
            let timeout = opts
                .as_ref()
                .map(|o| o.get::<Option<String>>("timeout"))
                .transpose()?
                .flatten()
                .and_then(|s| crate::model::parse_duration(&s))
                .unwrap_or(crate::barrier::DEFAULT_TIMEOUT);
            let root = std::env::current_dir().ok();
            let (path, position) = crate::barrier::join(&token, parties, root.as_deref())
                .map_err(mlua::Error::RuntimeError)?;

            let started = std::time::Instant::now();
            loop {
                if position >= parties
                    || crate::barrier::released(&path, &token, parties)
                        .map_err(mlua::Error::RuntimeError)?
                {
                    // The last one out leaves nothing behind, so the next barrier on this token
                    // counts from zero instead of inheriting a satisfied one.
                    if position >= parties {
                        crate::barrier::release(&path);
                    }
                    return Ok(position);
                }
                if started.elapsed() >= timeout {
                    return Err(mlua::Error::RuntimeError(crate::barrier::timeout_message(
                        &token,
                        parties,
                        crate::barrier::arrived(&path, &token),
                        started.elapsed(),
                    )));
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
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
    Ok(())
}

/// Typed resource constructors, named by the ACCESS MODE they take on a token: `writes` is an
/// exclusive (writer) hold, `reads` a concurrent (reader) one. Both accept a bare token *or* an
/// existing ref, so either can re-mode what the other made (`prova.reads(prova.port(5432))`).
/// `port` is exclusive — a listener is a writer of its port — and `reads` can widen it.
fn install_resources(lua: &Lua, prova: &Table) -> mlua::Result<()> {
    prova.set(
        "port",
        lua.create_function(|lua, (number, opts): (u64, Option<Table>)| {
            let machine = parse_lock_scope(&opts)?;
            lua.create_userdata(ResourceRef {
                token: format!("port:{number}"),
                shared: false,
                // A listener binds a HOST port, so the honest default scope is the machine:
                // two provas at one home already collide on :8080, and so do two homes.
                machine: machine.unwrap_or(true),
            })
        })?,
    )?;
    prova.set(
        "writes",
        lua.create_function(|lua, (v, opts): (Value, Option<Table>)| {
            let machine = parse_lock_scope(&opts)?;
            resource_ref(lua, v, false, machine)
        })?,
    )?;
    prova.set(
        "reads",
        lua.create_function(|lua, (v, opts): (Value, Option<Table>)| {
            let machine = parse_lock_scope(&opts)?;
            resource_ref(lua, v, true, machine)
        })?,
    )?;
    // The pre-`reads`/`writes` spellings, kept working but deliberately unadvertised: `resource` ==
    // `writes`, `shared` == `reads`. Their stubs are `---@deprecated`, which keeps an existing suite
    // resolving in the IDE while hiding them from `prova.help` — so nothing points a new author at
    // them, and no one's tests break the day they upgrade.
    prova.set(
        "resource",
        lua.create_function(|lua, v: Value| resource_ref(lua, v, false, None))?,
    )?;
    prova.set(
        "shared",
        lua.create_function(|lua, v: Value| resource_ref(lua, v, true, None))?,
    )?;
    Ok(())
}

/// The run's own facts, published for proofs to read: `prova.ports`, `prova.root`/`home`,
/// `prova.bin`, `prova.version`, and the `prova.help` surface.
fn install_run_facts(lua: &Lua, prova: &Table, config: &RunConfig) -> mlua::Result<()> {
    // The host port mode, readable by topology/plugin authors as `prova.ports` (`"auto"` | `"fixed"`).
    // `prova.containerized` consults it to upgrade random ports to fixed bindings under `--fixed`; a
    // recipe with an advertised listener (Kafka) reads it to emit the right listener address.
    prova.set("ports", config.ports.as_str())?;

    // Where the project is (`RunConfig::with_project`) — so a repo-local plugin can say
    // `prova.root .. "/target/debug/miniond"` instead of hardcoding an absolute path or trusting the
    // process cwd. `prova.root` and `prova.home` are synonyms for the project ROOT. Absent (nil) when
    // there is no manifest, e.g. a bare `prova <file>` run.
    if let Some(dir) = &config.project_dir {
        // /-normalized like every path-PRODUCING API (fs.tempdir, fs.glob, plugin.dir): discovery
        // canonicalizes, which on Windows grows the `\\?\` prefix, and a raw root breaks prefix
        // arithmetic against the normalized outputs (`hit:sub(#prova.root + 2)` over fs.glob).
        // `prova.bin` deliberately stays OS-native — it is used in command position, not as data.
        let dir = crate::modules::emit_path(dir);
        prova.set("root", dir.as_str())?;
        prova.set("home", dir.as_str())?;
    }

    // The prova binary running this suite (`RunConfig::with_prova_bin`), so a proof that drives prova
    // recursively names the build under test instead of whatever `PATH` resolves. Same argument as
    // `prova.root` one field up — anchor on what the runtime knows rather than trusting ambient
    // process state — applied to the executable instead of the project.
    //
    // What this guarantees is SELF-CONSISTENCY: the nested run is the same build as the run that
    // spawned it. That closes the split-brain case, where the suite runs from `target/` (`cargo xtask
    // test`/`run`) while its nested calls resolve to an installed `~/.cargo/bin/prova` belonging to
    // another checkout. It does NOT make an installed binary be the local build — invoke a stale
    // install and both layers are consistently stale, which is a provisioning problem, not one Lua
    // can see.
    //
    // Absent (nil) when the embedder supplied none; a proof then fails on a nil concat, which is the
    // honest outcome. There is deliberately no `PATH` fallback — a fallback would restore the silent
    // split this exists to remove.
    if let Some(bin) = &config.prova_bin {
        prova.set("bin", bin.to_string_lossy().as_ref())?;
    }

    // `prova.version` — the running version, as `--version` reports it, INCLUDING the `+dev.<sha>`
    // build metadata that marks a non-release build. A proof can therefore assert what it is
    // actually running on, which is the one thing that was missing when a local build and the
    // release it was cut from both claimed 0.11.0 and behaved differently.
    prova.set("version", crate::VERSION)?;

    // `prova.selection` — the run's resolved selection, as plain data
    // (docs/design/verifiers.md#selection-pushdown-into-conducts). The engine's whole
    // contribution to pushdown: a deputy's factory reads these axes and translates them to its
    // framework's own filter grammar (the knowledge belongs in the deputy's package, not in a
    // callback protocol); a deputy that ignores the table conducts in full. Every axis is
    // present (possibly empty), so consumers index without nil-guards.
    {
        let sel = &config.selection;
        let t = lua.create_table()?;
        let list = |v: &[String]| -> mlua::Result<Table> {
            let out = lua.create_table()?;
            for (i, s) in v.iter().enumerate() {
                out.set(i + 1, s.as_str())?;
            }
            Ok(out)
        };
        t.set("keywords", list(&sel.keywords)?)?;
        t.set("keyword_excludes", list(&sel.keyword_excludes)?)?;
        t.set("tags", list(&sel.tags)?)?;
        t.set("tag_excludes", list(&sel.tag_excludes)?)?;
        t.set("nodes", list(&sel.nodes)?)?;
        t.set("covering", list(&sel.covering)?)?;
        t.set("lane_tags", list(&sel.lane_tags)?)?;
        t.set("lane_tag_excludes", list(&sel.lane_tag_excludes)?)?;
        t.set("is_empty", sel.is_empty())?;
        prova.set("selection", t)?;
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
                crate::help::entries_with_packages(help_roots.iter().map(|p| p.as_path()));
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
    Ok(())
}

/// `runtime.*` — the companion's config DSL — is NOT available in a test/eval/topology state.
/// Accessing ANY member here raises a clear error instead of a baffling nil, because `runtime`
/// configures the environment tests run *in*, and only `prova.lua` loads early enough (with the
/// manifest, before any test) to do that. `load_project_config` overwrites this stub with the
/// working table when it loads the companion. Keeping it off `prova` — the authoring surface — is
/// what makes the boundary self-evident.
fn install_runtime_stub(lua: &Lua) -> mlua::Result<()> {
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
    Ok(())
}

/// `suite.config{ name?, requires? }` — configure the current suite (used in a `suite.lua`
/// setup file). `requires` gates the whole suite: it folds into the root node so every test
/// inherits it, and an unmet capability skips all the suite's files cleanly (skip, not fail).
/// `spec` is deliberately NOT accepted here: spec flags are test-level only — a suite-wide
/// flag recreates the graduation ceremony the revised design removed (api-freeze §5).
fn install_suite_config(lua: &Lua, col: &SharedCollector) -> mlua::Result<()> {
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
                // `switch` gates the whole suite as an opt-in class: it folds into the root node,
                // so every test inherits it and the suite is off unless the class is thrown
                // (docs/design/manifest.md#switches-not-env-capabilities).
                if let Some(s) = opts.get::<Option<String>>("switch")? {
                    if s.trim().is_empty() {
                        return Err(mlua::Error::RuntimeError(
                            "switch takes the opt-in class's name — `switch = \"ut\"`".into(),
                        ));
                    }
                    c.nodes[0].opts.switch = Some(s);
                }
                if !matches!(opts.get::<Value>("promises")?, Value::Nil) {
                    return Err(mlua::Error::RuntimeError(
                        "promises is test-level only — flag each open test, not the suite".into(),
                    ));
                }
                if !matches!(opts.get::<Value>("proves")?, Value::Nil) {
                    return Err(mlua::Error::RuntimeError(
                        "proves is test-level only — annotate each test, not the suite".into(),
                    ));
                }
                // The widest silent drop of all: a key ignored here mis-configures every file in
                // the suite. `promises`/`proves` are refused above with their own teaching, so by
                // here the only remaining keys are the accepted three or a mistake.
                super::collect::reject_unknown_opts(
                    &opts,
                    &["name", "requires", "switch"],
                    "suite.config",
                )?;
                Ok(())
            })?,
        )?;
        lua.globals().set("suite", suite)?;
    Ok(())
}

/// `prova.fixture(name, scope, factory, { identity = { command = …, inputs = { … } } })` — the
/// author's assertion that two conducts are the same question
/// (docs/design/agent-ergonomics.md#dedupe-identical-deputy-conducts).
///
/// Both halves are required, and the requirement is the safety property: a command with no inputs
/// cannot notice the tree changing, and inputs with no command would collapse two DIFFERENT tools
/// over one tree — handing one tool's verdict to the other's readers, which is a far worse failure
/// than paying for the conduct twice. Unknown keys are refused for the reason the DSL refuses them
/// anywhere (agent-ergonomics.md#unknown-test-opts-silently-ignored): a dropped option reads as
/// configured, and here it would silently mean "no sharing at all".
fn parse_identity(
    name: &str,
    scope: ScopeKind,
    opts: &Value,
) -> mlua::Result<Option<crate::engine::fixtures::IdentitySpec>> {
    let table = match opts {
        Value::Nil => return Ok(None),
        Value::Table(t) => t,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "fixture {name:?}: the 4th argument is an options table, got {}",
                other.type_name()
            )))
        }
    };
    crate::engine::collect::reject_unknown_opts(table, &["identity"], &format!("fixture {name:?}"))?;
    let Some(spec) = table.get::<Option<mlua::Table>>("identity")? else {
        return Ok(None);
    };
    crate::engine::collect::reject_unknown_opts(
        &spec,
        &["command", "inputs"],
        &format!("fixture {name:?} identity"),
    )?;
    if scope != ScopeKind::Run {
        return Err(mlua::Error::RuntimeError(format!(
            "fixture {name:?}: `identity` is Scope.Run's — it shares one EXECUTION across the run, \
             and a narrower scope already re-conducts per test/file/suite by definition"
        )));
    }
    let command: String = spec.get::<Option<String>>("command")?.ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "fixture {name:?} identity: `command` is required — it is what makes two identities the \
             SAME QUESTION rather than merely two conducts over one tree"
        ))
    })?;
    let inputs: Vec<String> = spec.get::<Option<Vec<String>>>("inputs")?.unwrap_or_default();
    if inputs.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "fixture {name:?} identity: `inputs` is required — without the files whose contents \
             change the answer, an identity cannot notice the tree changing"
        )));
    }
    Ok(Some(crate::engine::fixtures::IdentitySpec { command, inputs }))
}

/// The injection contract, installed LAST so none of prova's own setup writes pass through the
/// gate. The bundled modules are moved out of raw `_G` into (a) the canonical first-party surface
/// `prova.*` and (b) the `prova.namespaces` registry that `require` resolves. Ambient globals are
/// then whatever `[globals] inject` names — the core authoring globals `prova`/`Scope` always, other
/// modules only when injected, plus any injected PLUGINS (loaded eagerly, bound as bare globals,
/// NOT joined to `prova.*`). Moving names out of raw `_G` is what makes `__newindex` fire on
/// assignment at all; `__index` serves only the injected set, so a non-injected name reads as `nil`
/// (yet stays reachable as `prova.<name>` / `require("<name>")`) and is free for the user to assign.
fn install_injection(lua: &Lua, config: &RunConfig) -> mlua::Result<()> {
        let prova_tbl: Table = lua.globals().raw_get("prova")?;
        let all = lua.create_table()?; // name -> module, for require (bundled)
        let injected = lua.create_table()?; // name -> module, what ambient reads see
        // Names that raise on assignment: the injected set. `prova`/`Scope` are always injected.
        let mut injected_names: Vec<String> = vec!["prova".into(), "Scope".into()];

        for name in crate::RESERVED_NAMESPACES {
            let v: Value = lua.globals().raw_get(*name)?;
            if v.is_nil() {
                continue; // reserved but unshipped — nothing to serve
            }
            all.set(*name, v.clone())?;
            // First-party canonical surface: every bundled module under `prova.*` (not prova/Scope
            // themselves — those are the root, not fields of it).
            if *name != "prova" && *name != "Scope" {
                prova_tbl.set(*name, v.clone())?;
            }
            let inject_it = *name == "prova"
                || *name == "Scope"
                || config.globals_inject.iter().any(|e| e == name);
            if inject_it {
                injected.set(*name, v.clone())?;
                if *name != "prova" && *name != "Scope" {
                    injected_names.push((*name).to_string());
                }
            }
            lua.globals().raw_set(*name, Value::Nil)?;
        }
        lua.set_named_registry_value("prova.namespaces", all)?;

        // Injected PLUGINS: a name in the inject list that is not a bundled module is a declared
        // plugin. Eagerly `require` it (the searcher is already installed above) and bind it as a bare
        // unqualified global. Plugins do NOT join `prova.*` — a third party does not share prova's
        // namespace.
        let require: mlua::Function = lua.globals().get("require")?;
        for name in &config.globals_inject {
            if crate::is_injectable_module(name) {
                continue; // a bundled module, handled above
            }
            let m: Value = require.call(name.clone())?;
            injected.set(name.as_str(), m)?;
            injected_names.push(name.clone());
        }

        let mt = lua.create_table()?;
        mt.set("__index", injected)?;
        mt.set(
            "__newindex",
            lua.create_function(move |_, (t, k, v): (Table, Value, Value)| {
                if let Value::String(s) = &k {
                    let name = s.to_string_lossy();
                    let name: &str = &name;
                    if injected_names.iter().any(|n| n.as_str() == name) {
                        return Err(mlua::Error::RuntimeError(format!(
                            "cannot assign to '{name}' — it is an injected prova namespace; use a \
                             local, or drop it from [globals] inject"
                        )));
                    }
                }
                t.raw_set(k, v)?;
                Ok(())
            })?,
        )?;
        lua.globals().set_metatable(Some(mt))?;
    Ok(())
}

pub(super) fn build_lua(root_name: String, config: &RunConfig) -> mlua::Result<(Lua, SharedCollector)> {
    let col: SharedCollector = Rc::new(RefCell::new(Collector::new(root_name)));
    let lua = Lua::new();
    install_os_env(&lua)?;

    let prova = lua.create_table()?;
    install_registration(&lua, &prova, &col)?;
    install_fixtures(&lua, &prova, &col)?;
    install_remind(&lua, &prova, &col)?;
    install_utilities(&lua, &prova)?;
    install_resources(&lua, &prova)?;
    install_run_facts(&lua, &prova, config)?;
    lua.globals().set("prova", prova)?;

    install_runtime_stub(&lua)?;
    // The typed fixture-scope constants: `Scope.Test` / `Scope.Flow` / `Scope.File` / `Scope.Suite`.
    lua.globals().set("Scope", make_scope_global(&lua)?)?;
    install_suite_config(&lua, &col)?;

    // First-party capability modules (`shell`, `fs`) as their own injected globals.
    crate::modules::install(
        &lua,
        config.progress(),
        config.deputed_registry.clone(),
        config.measurement_registry.clone(),
        config.report_registry.clone(),
        config.report_custody.clone(),
    )?;

    // Host-provided plugin modules (e.g. `archetect`), installed into every Lua state.
    for install in &config.modules {
        install(&lua)?;
    }

    // Wire `require` to resolve Lua plugins (bundled + manifest + disk). Installed last so a plugin
    // loaded via `require` sees every primitive global it composes.
    //
    // Search roots are exactly what the embedder declared (`with_package_root`) — the engine adds
    // none of its own. It used to join `<project_root>/.prova/plugins` here, which meant the answer
    // to "where do plugins come from?" was split between this file and the manifest. The CLI now
    // passes the manifest's `[run] package_root` (already absolutised against the project root), so
    // the manifest is the single, readable source of truth and the engine has no layout opinion.
    crate::packages::install(
        &lua,
        &config.package_roots,
        &config.named_packages,
        &config.package_namespaces,
    )?;
    install_injection(&lua, config)?;

    Ok((lua, col))
}

pub(super) struct GroupBuilder {
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
pub(super) fn caller_line(lua: &Lua, col: &SharedCollector) -> Option<u32> {
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
pub(super) fn register_test(
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
    case: Option<Value>,
    line: Option<u32>,
) -> mlua::Result<NodeIx> {
    let (opts, body, falsifier) = split_opts_body(a, b, "test", &name)?;
    Ok(col.borrow_mut().add(
        parent,
        Node {
            name,
            kind: NodeKind::Test,
            falsifier,
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
pub(super) fn register_test_each(
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
pub(super) fn render_case_name(template: &str, case: &Value) -> mlua::Result<String> {
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
pub(super) fn value_to_string(v: &Value) -> String {
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
pub(super) fn register_group(
    lua: &Lua,
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
) -> mlua::Result<NodeIx> {
    let (opts, body, falsifier) = split_opts_body(a, b, "group", &name)?;
    reject_falsifier(falsifier, "group")?;
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
            falsifier: None,
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
pub(super) fn register_describe(
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
                falsifier: None,
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
pub(super) fn register_flow(
    lua: &Lua,
    col: &SharedCollector,
    parent: NodeIx,
    name: String,
    a: Value,
    b: Value,
) -> mlua::Result<NodeIx> {
    let (opts, body, falsifier) = split_opts_body(a, b, "flow", &name)?;
    reject_falsifier(falsifier, "flow")?;
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
            falsifier: None,
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
pub(super) struct FlowBuilder {
    col: SharedCollector,
    ix: NodeIx,
}

impl UserData for FlowBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("step", |lua, this, (name, a, b): (String, Value, Value)| {
            let (opts, body, falsifier) = split_opts_body(a, b, "flow(…):step", &name)?;
            let line = caller_line(lua, &this.col);
            this.col.borrow_mut().add(
                this.ix,
                Node {
                    name,
                    kind: NodeKind::Test,
                    falsifier,
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
