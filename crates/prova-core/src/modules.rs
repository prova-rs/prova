//! First-party capability modules injected as globals alongside `prova`.
//!
//! These are what make prova useful beyond testing itself — bring a system into existence and poke
//! it. Two kinds live here, and the split is deliberate:
//!
//! - **Primitives + substrate** (always the foundation): `shell.run`/`shell.spawn`, `fs`, `net`,
//!   `docker` (the typed **bollard** daemon client). `shell`/`docker` are async (child processes /
//!   docker calls never block the worker); `fs` is synchronous. All take context explicitly (no
//!   ambient cwd), preserving the isolation the design promises.
//! - **Network-drive clients** — `http`, `grpc`, `graphql` — how you *drive the app under test*
//!   (there is no CLI-in-image for arbitrary gRPC), plus `yaml` (a parse util) and `sqlite` (the one
//!   embedded, no-docker database). Each is behind a default-on feature so a build can opt out.
//!
//! **Resource clients are NOT here.** Databases, caches, brokers, object stores, streams — every
//! *containerized* resource — are **external docker-exec plugins** (`prova-rs/prova-<name>`, authored
//! through `prova.containerized` + `container:run`), fetched via `prova.toml`, not compiled in. That
//! keeps the binary lean and privileges no technology. Modules that need docker declare
//! `requires = { "docker" }` to skip gracefully where the daemon is absent.

use std::path::Path;
use std::time::Instant;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::model::parse_duration;

/// Install the built-in module globals (`shell`, `fs`, `docker`, and — with the `http` feature —
/// `http`) into `lua`.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set("shell", make_shell(lua)?)?;
    lua.globals().set("fs", make_fs(lua)?)?;
    lua.globals().set("net", make_net(lua)?)?;
    // `prova.parse.*` — the exec-CLI output-parsing toolkit (lines / rows / table / json), added to
    // the `prova` global built earlier in build_lua. Broadly useful, so it lives at the root.
    {
        let prova: Table = lua.globals().get("prova")?;
        prova.set("parse", make_parse(lua)?)?;
    }
    #[cfg(feature = "docker")]
    lua.globals().set("docker", docker::make(lua)?)?;
    #[cfg(feature = "http")]
    lua.globals().set("http", http::make(lua)?)?;
    #[cfg(feature = "sqlite")]
    lua.globals()
        .set("sqlite", sql::make(lua, sql::Engine::Sqlite)?)?;
    #[cfg(feature = "grpc")]
    lua.globals().set("grpc", grpc::make(lua)?)?;
    #[cfg(feature = "graphql")]
    lua.globals().set("graphql", graphql::make(lua)?)?;
    #[cfg(feature = "yaml")]
    lua.globals().set("yaml", yaml::make(lua)?)?;
    // Absent-namespace stubs: in a lean distribution a native namespace's feature may be off. Install
    // a stub so `kafka.client(...)` raises a clear "not compiled into this build" error instead of a
    // bare `attempt to index a nil value` — the call-side companion to the `requires` skip. In the
    // default build every feature is on, so none of these arms compile.
    #[cfg(not(feature = "docker"))]
    lua.globals().set("docker", absent_stub(lua, "docker")?)?;
    #[cfg(not(feature = "http"))]
    lua.globals().set("http", absent_stub(lua, "http")?)?;
    #[cfg(not(feature = "sqlite"))]
    lua.globals().set("sqlite", absent_stub(lua, "sqlite")?)?;
    #[cfg(not(feature = "grpc"))]
    lua.globals().set("grpc", absent_stub(lua, "grpc")?)?;
    #[cfg(not(feature = "graphql"))]
    lua.globals().set("graphql", absent_stub(lua, "graphql")?)?;
    #[cfg(not(feature = "yaml"))]
    lua.globals().set("yaml", absent_stub(lua, "yaml")?)?;
    // The `prova.containerized` scaffolding helper — the ergonomic keystone every containerized
    // resource (first-party recipe or third-party plugin) is authored through. Always available;
    // the globals it composes (`docker`, `prova.retry`) resolve when a generated `container` is
    // *called*. Loaded before the recipes so they can be expressed in terms of it.
    lua.load(CONTAINERIZED_LUA)
        .set_name("@prova/containerized")
        .exec()?;
    // Resource recipes — Lua sugar over docker.run + prova.retry + a client + ctx:manage. Loaded
    // after the modules exist; the globals they touch resolve when a recipe is *called*.
    Ok(())
}

/// `prova.parse.*` — the exec-CLI output-parsing toolkit. A docker-exec plugin drives a CLI and gets
/// text back; these turn the common shapes into Lua values, so plugins never hand-roll parsing:
/// `lines` (line-oriented), `rows`/`table` (delimited — TSV/psql `|`/CSV), `json` (JSON, incl. the
/// one-object-per-line `--json` streams many CLIs emit, via `lines` + `json`).
fn make_parse(lua: &Lua) -> mlua::Result<Table> {
    let parse = lua.create_table()?;

    // lines(s) → non-empty, trimmed lines.
    parse.set(
        "lines",
        lua.create_function(|lua, s: String| {
            let out: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            lua.create_sequence_from(out)
        })?,
    )?;

    // rows(s, sep?) → a list of rows, each a list of columns split on `sep` (default tab). Blank
    // lines are skipped.
    parse.set(
        "rows",
        lua.create_function(|lua, (s, sep): (String, Option<String>)| {
            let sep = sep.unwrap_or_else(|| "\t".to_string());
            let rows = lua.create_table()?;
            for (i, line) in s.lines().filter(|l| !l.is_empty()).enumerate() {
                rows.set(i + 1, lua.create_sequence_from(line.split(&sep))?)?;
            }
            Ok(rows)
        })?,
    )?;

    // table(s, sep?) → the first non-empty line is a header row; each remaining row becomes a map
    // keyed by header name (the "column by header" shape, e.g. rabbitmqadmin's TSV).
    parse.set(
        "table",
        lua.create_function(|lua, (s, sep): (String, Option<String>)| {
            let sep = sep.unwrap_or_else(|| "\t".to_string());
            let mut non_empty = s.lines().filter(|l| !l.is_empty());
            let headers: Vec<&str> = match non_empty.next() {
                Some(h) => h.split(&sep).collect(),
                None => return lua.create_table(),
            };
            let rows = lua.create_table()?;
            for (i, line) in non_empty.enumerate() {
                let cols: Vec<&str> = line.split(&sep).collect();
                let row = lua.create_table()?;
                for (j, h) in headers.iter().enumerate() {
                    row.set(*h, *cols.get(j).unwrap_or(&""))?;
                }
                rows.set(i + 1, row)?;
            }
            Ok(rows)
        })?,
    )?;

    // json(s) → a Lua value. A real JSON parse (top-level `null` → `nil`, unlike a raw serde bridge).
    parse.set(
        "json",
        lua.create_function(|lua, s: String| {
            let v: serde_json::Value = serde_json::from_str(&s)
                .map_err(|e| mlua::Error::RuntimeError(format!("prova.parse.json: {e}")))?;
            json_value_to_lua(lua, &v)
        })?,
    )?;

    Ok(parse)
}

/// Convert a `serde_json::Value` to a Lua value, mapping JSON `null` to Lua `nil` (so an absent
/// field reads as nil, not a null sentinel).
fn json_value_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<mlua::Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(0.0)),
        },
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(a) => {
            let t = lua.create_table()?;
            for (i, item) in a.iter().enumerate() {
                t.set(i + 1, json_value_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, val) in o {
                t.set(k.as_str(), json_value_to_lua(lua, val)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A stand-in for a native namespace whose feature was not compiled into this build: any field
/// access raises a clear, actionable error instead of a bare `attempt to index a nil value`. A test
/// that wants to *skip* rather than error should gate with `requires = { "<name>" }`.
///
/// `#[allow(dead_code)]`: only referenced by the `#[cfg(not(feature = …))]` install arms, so in a
/// default (all-features) build it compiles but is never called.
#[allow(dead_code)]
fn absent_stub(lua: &Lua, name: &'static str) -> mlua::Result<Table> {
    let tbl = lua.create_table()?;
    let mt = lua.create_table()?;
    let index = lua.create_function(move |_, (_t, key): (Table, mlua::String)| {
        let key = key.to_string_lossy();
        Err::<mlua::Value, _>(mlua::Error::RuntimeError(format!(
            "`{name}.{key}` is unavailable: the `{name}` capability is not compiled into this build \
             (use a distribution that includes it, or gate the test with requires = {{ \"{name}\" }} \
             to skip instead)"
        )))
    })?;
    mt.set("__index", index)?;
    tbl.set_metatable(Some(mt))?;
    Ok(tbl)
}

/// `prova.containerized(spec)` — build a grammar-conformant namespace (`{ client?, container }`) from
/// a compact spec, so first-party recipes and third-party plugins are authored the same way and come
/// out the same shape (the tier-agnostic interface — see docs/design/ecosystem.md).
///
/// The generated `container(ctx, opts?)` provisions via `docker.run`, waits for readiness, ties
/// teardown to the scope with `ctx:manage`, and returns `{ url, container }` — attaching a managed
/// `client` (via `prova.retry`) only when the spec provides a `client` factory, so provisioning works
/// even where the native client is absent (§ black-box). `opts` overrides `image`/`tag`/`timeout`/`env`
/// at call time; `env`/`url`/`client` may read `opts`.
///
/// The same recipe expresses the **system under test**: give it `build` instead of `image` and its
/// image is built from the project's own Dockerfile rather than pulled. Nothing else changes — a SUT
/// is not a separate concept, it is a resource whose image happens to be local, so it inherits the
/// topology auto-join, the network vantage, readiness and teardown unchanged. That is what lets a
/// suite drop `requires = { "dotnet" }` for `requires = { "docker" }` and still test the real
/// production artifact (see docs/design/topologies.md).
///
/// Spec fields: `name` (for messages), `image` (base repo, pulled) **or** `build` (built — a
/// `{ context, dockerfile?, tag?, buildargs?, target?, pull?, nocache? }` table, or a bare string as
/// shorthand for `{ context = … }`), `tag` (default tag; pulled images only), `port`/`ports`
/// (published; `port` is the primary for readiness + url; a `ports` entry may be a number for a
/// random host port or `{ container, host }` for a fixed one), `command?`, `env?` (table or
/// `function(opts)->table`), `wait?` (`{ port|log }`, default `{ port = primary }`), `timeout?`,
/// `url` (`function(host_port, opts)->string`, required), `client?`
/// (`function(url, opts, container)->handle` — the `container` is passed so a docker-exec client can
/// `exec` into it; a native client just uses `url`), `extra?` (`function(url, opts, container)->table`
/// of additional resource fields beyond the trio, e.g. s3 credentials).
const CONTAINERIZED_LUA: &str = r#"
function prova.containerized(spec)
  assert(type(spec) == "table", "prova.containerized: pass a spec table")
  assert((spec.image or spec.build) and spec.url,
         "prova.containerized: spec needs `image` (pulled) or `build` (built), and `url`")
  assert(not (spec.image and spec.build),
         "prova.containerized: spec has both `image` and `build` — an image is pulled or built, not both")
  local name = spec.name or "resource"
  local ports = spec.ports
  if type(ports) == "number" then ports = { ports } end
  ports = ports or { spec.port }
  -- The primary container port (for readiness + url). A `ports` entry may be a plain number (random
  -- host port) or a `{ container = N, host = M }` table (fixed host port, e.g. Kafka's advertised
  -- listener), which is passed through to docker.run verbatim.
  local primary = spec.port
  if not primary and ports[1] then
    primary = type(ports[1]) == "table" and ports[1].container or ports[1]
  end
  assert(primary, "prova.containerized: spec needs a `port` (or `ports`)")

  -- Port mode (set by the verb): tests and `prova up` default to random host ports (parallel-safe,
  -- collision-free). `prova up --fixed` sets `prova.ports == "fixed"`, which pins each *random* entry
  -- to its canonical container port so external tools connect on a predictable address. Entries the
  -- author already fixed (`{ container, host }`) are left exactly as written.
  if prova.ports == "fixed" then
    local pinned = {}
    for i, p in ipairs(ports) do
      if type(p) == "number" then
        pinned[i] = { container = p, host = p }
      else
        pinned[i] = p
      end
    end
    ports = pinned
  end

  local ns = { client = spec.client }

  function ns.container(ctx, opts)
    assert(ctx and ctx.manage, name .. ".container(ctx, opts?): pass the fixture/test context first")
    opts = opts or {}

    -- The image is either PULLED (`spec.image`, a published resource) or BUILT (`spec.build`, the
    -- system under test from its own Dockerfile). A built image is the ONLY difference between a SUT
    -- and any other resource: everything downstream — ports, env, the network auto-join, the vantage
    -- swap, readiness, teardown — is identical, which is the point. `opts.image` still overrides
    -- either, so a caller can pin a prebuilt artifact (e.g. an image CI already published).
    local image = opts.image
    if not image then
      if spec.build then
        local b = spec.build
        if type(b) == "string" then b = { context = b } end   -- `build = "."` shorthand
        image = docker.build{
          context = b.context, dockerfile = b.dockerfile, tag = b.tag,
          buildargs = b.buildargs, target = b.target, pull = b.pull, nocache = b.nocache,
        }
      else
        image = spec.image
        local tag = opts.tag or spec.tag
        if tag then image = image .. ":" .. tag end
      end
    end
    local timeout = opts.timeout or spec.timeout or "60s"

    local env = opts.env
    if env == nil then
      env = spec.env
      if type(env) == "function" then env = env(opts) end
    end

    local w = spec.wait or { port = primary }
    local wait = { port = w.port, log = w.log, timeout = timeout }

    -- `network`/`alias` (from a topology) join the container to a user-defined network so an
    -- in-network consumer — a containerized SUT — can reach it by DNS. Host publishing is unchanged,
    -- so the resource is dual-homed.
    --
    -- The topology convenience: a `prova.topology` factory exposes an ambient managed network on
    -- `ctx.network`. When the author wrote no explicit `network`, a resource provisioned there
    -- auto-joins that network, aliased by its recipe `name`. Explicit `opts.network` still wins, and
    -- `ctx.network` is nil in ordinary fixtures (and test bodies), so those resources are entirely
    -- unaffected — no network is created and no `.network` field is added.
    local network = opts.network
    local alias = opts.alias
    if network == nil and ctx.network ~= nil then
      network = ctx.network
      alias = alias or name
    end

    -- `host.docker.internal` reaches the host from inside the container: on native Linux via the
    -- `host-gateway` mapping, on Docker Desktop it is provided already (so this is a no-op there).
    -- Passed unconditionally so a containerized SUT can reach a host-bound `http.mock`/`grpc.mock`
    -- (its `.network.url`) without the author threading anything through — one code path, both
    -- platforms. An author-supplied `extra_hosts` is preserved and this is appended.
    local extra_hosts = { "host.docker.internal:host-gateway" }
    if opts.extra_hosts then
      for _, h in ipairs(opts.extra_hosts) do table.insert(extra_hosts, h) end
    end

    local container = ctx:manage(docker.run{
      image = image, ports = ports, env = env, command = spec.command, wait = wait,
      network = network, alias = alias, extra_hosts = extra_hosts,
    })

    local hp = container:host_port(primary)
    local url = spec.url(hp, opts)
    -- The standard resource shape: client/url/container, plus the primary endpoint split out as
    -- host/port so env wiring is `DbHost = res.host, DbPort = res.port` — no host_port() ceremony.
    local res = { url = url, container = container, host = "127.0.0.1", port = hp }

    -- The network vantage: when joined with an alias, expose the address an in-network consumer
    -- uses — the alias + the CONTAINER port (not the mapped host port), and the url rewritten from
    -- the host authority to the network authority. `resource.network = { url, host, port, alias }`.
    if alias then
      local host_authority = "127.0.0.1:" .. hp
      local net_authority = alias .. ":" .. primary
      local at = url:find(host_authority, 1, true)   -- plain find (no Lua-pattern surprises)
      local net_url = at and (url:sub(1, at - 1) .. net_authority .. url:sub(at + #host_authority)) or url
      res.network = { url = net_url, host = alias, port = primary, alias = alias }
    end
    -- Extra resource fields beyond the trio (e.g. s3 credentials): `spec.extra(url, opts, container)`
    -- returns a table merged into the result. The reserved names are `client`/`url`/`container`/`host`/`port`.
    if type(spec.extra) == "function" then
      for k, v in pairs(spec.extra(url, opts, container)) do
        if k ~= "client" and k ~= "url" and k ~= "container" and k ~= "host" and k ~= "port" then res[k] = v end
      end
    end
    if spec.client then
      -- The factory gets the container too, so a docker-exec client (no native driver) can `exec`
      -- into it; a native client just uses `url` and ignores the extra arg.
      res.client = ctx:manage(prova.retry(function() return spec.client(url, opts, container) end,
        { timeout = timeout, message = name .. " did not become ready in time" }))
    end
    return res
  end

  return ns
end
"#;

// ---------------------------------------------------------------------------------------------
// shell
// ---------------------------------------------------------------------------------------------

/// Result of `shell.run` — field access (`r.code`, `r.stdout`) plus `r:ok()`.
struct ShellResult {
    code: i32,
    stdout: String,
    stderr: String,
    duration: f64,
}

impl UserData for ShellResult {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("code", |_, this| Ok(this.code));
        fields.add_field_method_get("stdout", |_, this| Ok(this.stdout.clone()));
        fields.add_field_method_get("stderr", |_, this| Ok(this.stderr.clone()));
        fields.add_field_method_get("duration", |_, this| Ok(this.duration));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("ok", |_, this, ()| Ok(this.code == 0));
    }
}

/// A long-running process started by `shell.spawn` — the primitive for "boot the app, test it, stop
/// it". `proc.pid`, `proc:running()`, `proc:stop()` (async), `proc:wait()` (async). `kill_on_drop`
/// is a backstop, but the blessed pattern is `ctx:defer(function() proc:stop() end)` so the process
/// is reaped during (async) teardown while the runtime is still alive.
struct Process {
    child: Option<tokio::process::Child>,
    pid: Option<u32>,
    // Combined stdout+stderr, captured by reader tasks into a bounded buffer (oldest dropped),
    // so a failed boot is never blind: `proc:output()` returns what the app said.
    output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

/// Cap for a spawned process's captured output. Old bytes drop first.
const SPAWN_OUTPUT_CAP: usize = 64 * 1024;

fn spawn_output_reader(
    stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
) {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut stream = stream;
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut b = buf.lock().unwrap_or_else(|p| p.into_inner());
                    b.extend_from_slice(&chunk[..n]);
                    if b.len() > SPAWN_OUTPUT_CAP {
                        let overflow = b.len() - SPAWN_OUTPUT_CAP;
                        b.drain(..overflow);
                    }
                }
            }
        }
    });
}

impl UserData for Process {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("pid", |_, this| Ok(this.pid));
    }
    // NOTE: output() lives in add_methods below.
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // The process's combined stdout+stderr so far (bounded: last 64KB, oldest dropped).
        // The escape hatch for blind boots and the hook for asserting on log output.
        methods.add_method("output", |_, this, ()| {
            let buf = this.output.lock().unwrap_or_else(|p| p.into_inner());
            Ok(String::from_utf8_lossy(&buf).into_owned())
        });
        // Kill (SIGKILL) and reap. Idempotent — a second stop, or stop after exit, is a no-op.
        methods.add_async_method_mut("stop", |_, mut this, ()| async move {
            if let Some(mut child) = this.child.take() {
                let _ = child.kill().await;
            }
            Ok(())
        });
        // Wait for exit; returns the exit code (or nil if killed by a signal / already reaped).
        methods.add_async_method_mut("wait", |_, mut this, ()| async move {
            match this.child.take() {
                Some(mut child) => {
                    let status = child.wait().await.map_err(|e| {
                        mlua::Error::RuntimeError(format!("process wait failed: {e}"))
                    })?;
                    Ok(status.code())
                }
                None => Ok(None),
            }
        });
        // Is the process still running? Reaps it if it has already exited.
        methods.add_method_mut("running", |_, this, ()| {
            let running = match &mut this.child {
                Some(child) => matches!(child.try_wait(), Ok(None)),
                None => false,
            };
            if !running {
                this.child = None;
            }
            Ok(running)
        });
    }
}

fn make_shell(lua: &Lua) -> mlua::Result<Table> {
    let shell = lua.create_table()?;
    shell.set(
        "run",
        lua.create_async_function(|lua, (cmd, opts): (mlua::Value, Option<Table>)| async move {
            // Extract options up front (owned) so nothing borrows Lua across the await.
            let cmd = CommandSpec::parse(cmd)?;
            let cwd = opt_string(&opts, "cwd")?;
            let env = opt_env(&opts)?;
            let timeout = opt_string(&opts, "timeout")?.and_then(|s| parse_duration(&s));
            let check = opts
                .as_ref()
                .map(|o| o.get::<Option<bool>>("check"))
                .transpose()?
                .flatten()
                .unwrap_or(false);

            // A string runs through a shell (`"cargo build --release"` verbatim); an argv table runs
            // the program directly — no shell, no quoting.
            let mut command = cmd.build();
            if let Some(dir) = &cwd {
                command.current_dir(dir);
            }
            for (k, v) in &env {
                command.env(k, v);
            }

            let start = Instant::now();
            let run = command.output();
            let output = match timeout {
                Some(budget) => tokio::time::timeout(budget, run).await.map_err(|_| {
                    mlua::Error::RuntimeError(format!(
                        "shell.run timed out after {budget:?}: {cmd}"
                    ))
                })?,
                None => run.await,
            }
            .map_err(|e| mlua::Error::RuntimeError(format!("shell.run failed to spawn: {e}")))?;

            let result = ShellResult {
                code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                duration: start.elapsed().as_secs_f64(),
            };
            if check && result.code != 0 {
                // Builds put failure detail on either stream (msbuild/pnpm favor stdout), so the
                // error carries the tail of both — better than any hand-rolled assert.
                return Err(mlua::Error::RuntimeError(format!(
                    "shell.run: command exited {} (check=true): {cmd}\n--- stderr ---\n{}\n--- stdout ---\n{}",
                    result.code,
                    tail(&result.stderr, 4096),
                    tail(&result.stdout, 4096)
                )));
            }
            lua.create_userdata(result)
        })?,
    )?;

    // shell.spawn(cmd, { cwd, env }) → a Process handle for a long-running command (a booted app,
    // a mock server). stdout/stderr are discarded in v1. Called inside prova's runtime, so the
    // tokio process driver is available.
    shell.set(
        "spawn",
        lua.create_function(|lua, (cmd, opts): (mlua::Value, Option<Table>)| {
            let cmd = CommandSpec::parse(cmd)?;
            let cwd = opt_string(&opts, "cwd")?;
            let env = opt_env(&opts)?;
            let mut command = cmd.build();
            if let Some(dir) = &cwd {
                command.current_dir(dir);
            }
            for (k, v) in &env {
                command.env(k, v);
            }
            command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let mut child = command
                .spawn()
                .map_err(|e| mlua::Error::RuntimeError(format!("shell.spawn failed: {e}")))?;
            let pid = child.id();
            let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            if let Some(out) = child.stdout.take() {
                spawn_output_reader(out, output.clone());
            }
            if let Some(err) = child.stderr.take() {
                spawn_output_reader(err, output.clone());
            }
            lua.create_userdata(Process {
                child: Some(child),
                pid,
                output,
            })
        })?,
    )?;

    Ok(shell)
}

/// What to run: a **string** (routed through a shell, so `"cargo build --release"` works verbatim)
/// or an **argv table** (`{"psql", "-tAc", sql}` — no shell, no quoting), mirroring `container:run`.
///
/// The argv form is what makes passing *content* to a local CLI safe — SQL, Lua source, JSON, a
/// path with spaces. There is no quoting layer to get wrong, so there is nothing to get wrong. Its
/// absence previously forced authors to route around the API (write the payload to a temp file and
/// pass a path) for the local half of an SDK whose containerized half had argv all along. See
/// `docs/design/agent-ergonomics.md` §1.
enum CommandSpec {
    Shell(String),
    Argv(Vec<String>),
}

impl CommandSpec {
    fn parse(v: mlua::Value) -> mlua::Result<Self> {
        match v {
            mlua::Value::String(s) => Ok(Self::Shell(s.to_str()?.to_string())),
            mlua::Value::Table(t) => {
                let argv: Vec<String> = t.sequence_values::<String>().collect::<mlua::Result<_>>().map_err(
                    |e| mlua::Error::RuntimeError(format!("argv entries must all be strings: {e}")),
                )?;
                if argv.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        r#"argv table is empty — expected { "program", "arg", … }"#.into(),
                    ));
                }
                Ok(Self::Argv(argv))
            }
            other => Err(mlua::Error::RuntimeError(format!(
                "command must be a string (run via a shell) or an argv table (no shell, no quoting), got {}",
                other.type_name()
            ))),
        }
    }

    fn build(&self) -> tokio::process::Command {
        match self {
            Self::Shell(s) => shell_command(s),
            Self::Argv(argv) => {
                let mut c = tokio::process::Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            }
        }
    }
}

/// How the command reads back in an error — the argv form joined for legibility (it is a display,
/// not a re-runnable quoting).
impl std::fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell(s) => f.write_str(s),
            Self::Argv(argv) => f.write_str(&argv.join(" ")),
        }
    }
}

/// The shell that a *string* command is routed through. Two definitions, one per platform — so each
/// needs its own `cfg`: without one on this arm it is compiled on Windows too, and collides with the
/// Windows arm. (`CommandSpec` above must NOT be gated: it is platform-independent, and gating it was
/// what made Windows fail to compile at all.)
#[cfg(unix)]
fn shell_command(cmd: &str) -> tokio::process::Command {
    let mut c = tokio::process::Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

#[cfg(windows)]
fn shell_command(cmd: &str) -> tokio::process::Command {
    let mut c = tokio::process::Command::new("cmd");
    c.arg("/C").arg(cmd);
    c
}

fn opt_string(opts: &Option<Table>, key: &str) -> mlua::Result<Option<String>> {
    match opts {
        Some(t) => t.get::<Option<String>>(key),
        None => Ok(None),
    }
}

fn opt_env(opts: &Option<Table>) -> mlua::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    if let Some(t) = opts {
        if let Some(env) = t.get::<Option<Table>>("env")? {
            for pair in env.pairs::<String, Value>() {
                let (k, v) = pair?;
                let value = env_value(&k, v)?;
                out.push((k, value));
            }
        }
    }
    Ok(out)
}

/// Environment values coerce from the scalars tests naturally hold — ports are numbers, flags are
/// booleans — so suites never write `tostring()` around env wiring.
fn env_value(key: &str, v: Value) -> mlua::Result<String> {
    Ok(match v {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => {
            // Render integral floats without a trailing .0 (Lua numbers are f64).
            if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
                format!("{}", n as i64)
            } else {
                n.to_string()
            }
        }
        Value::Boolean(b) => b.to_string(),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "env.{key}: expected string/number/boolean, got {}",
                other.type_name()
            )))
        }
    })
}

/// Last `max` bytes of `s`, on a char boundary, prefixed with an ellipsis marker when truncated.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("[... truncated ...]\n{}", &s[start..])
}

// ---------------------------------------------------------------------------------------------
// fs
// ---------------------------------------------------------------------------------------------

fn make_fs(lua: &Lua) -> mlua::Result<Table> {
    let fs = lua.create_table()?;

    fs.set(
        "exists",
        lua.create_function(|_, path: String| Ok(Path::new(&path).exists()))?,
    )?;

    fs.set(
        "read",
        lua.create_function(|_, path: String| {
            std::fs::read_to_string(&path)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.read {path:?}: {e}")))
        })?,
    )?;

    fs.set(
        "write",
        lua.create_function(|_, (path, contents): (String, String)| {
            if let Some(parent) = Path::new(&path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| mlua::Error::RuntimeError(format!("fs.write {path:?}: {e}")))?;
            }
            std::fs::write(&path, contents)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.write {path:?}: {e}")))
        })?,
    )?;

    fs.set(
        "remove_all",
        lua.create_function(|_, path: String| {
            let p = Path::new(&path);
            let result = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match result {
                Ok(()) => Ok(()),
                // Removing something already gone is a no-op, not an error.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(mlua::Error::RuntimeError(format!(
                    "fs.remove_all {path:?}: {e}"
                ))),
            }
        })?,
    )?;

    fs.set(
        "tempdir",
        lua.create_function(|_, ()| {
            crate::engine::make_tempdir()
                .map(|p| p.to_string_lossy().into_owned())
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.tempdir: {e}")))
        })?,
    )?;

    // fs.glob(root, "**/*.rs") → sorted list of matching paths (as strings).
    fs.set(
        "glob",
        lua.create_function(|lua, (root, pattern): (String, String)| {
            let joined = Path::new(&root).join(&pattern);
            let pattern = joined.to_string_lossy();
            let paths = glob::glob(&pattern)
                .map_err(|e| mlua::Error::RuntimeError(format!("fs.glob {pattern:?}: {e}")))?;
            let mut out: Vec<String> = paths
                .filter_map(|r| r.ok())
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            out.sort();
            lua.create_sequence_from(out)
        })?,
    )?;

    Ok(fs)
}

// ---------------------------------------------------------------------------------------------
// net
// ---------------------------------------------------------------------------------------------

fn make_net(lua: &Lua) -> mlua::Result<Table> {
    let net = lua.create_table()?;

    // net.free_port() → an OS-assigned free TCP port on 127.0.0.1. Bind to :0, read the assigned
    // port, and release it. The classic use is a dynamic port for a locally `shell.spawn`ed app (a
    // container gets its random host port from `docker.run` instead). There is an inherent race —
    // the port is free *now*, not guaranteed still free when the app binds — but in practice the
    // window is tiny and this is the standard approach.
    net.set(
        "free_port",
        lua.create_function(|_, ()| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                .map_err(|e| mlua::Error::RuntimeError(format!("net.free_port: {e}")))?;
            let port = listener
                .local_addr()
                .map_err(|e| mlua::Error::RuntimeError(format!("net.free_port: {e}")))?
                .port();
            Ok(port)
        })?,
    )?;

    Ok(net)
}

// ---------------------------------------------------------------------------------------------
// http (async; HTTP-only in v1 — https lands behind a later `tls` feature)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "http")]
mod http {
    use std::time::{Duration, Instant};

    use mlua::{
        Function, Lua, LuaSerdeExt, Table, UserData, UserDataFields, UserDataMethods, Value,
    };

    use crate::model::parse_duration;

    /// A response from the `http` module: `res.status`, `res.body`, `res.headers`, `res:json()`.
    struct HttpResponse {
        status: u16,
        body: String,
        headers: Vec<(String, String)>,
    }

    impl UserData for HttpResponse {
        fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
            fields.add_field_method_get("status", |_, this| Ok(this.status));
            fields.add_field_method_get("body", |_, this| Ok(this.body.clone()));
            fields.add_field_method_get("headers", |lua, this| {
                let table = lua.create_table()?;
                for (k, v) in &this.headers {
                    table.set(k.clone(), v.clone())?;
                }
                Ok(table)
            });
        }
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // Decode the body as JSON into a Lua value; raises on non-JSON. JSON nulls become
            // Lua nil (not mlua's null sentinel) so `t:expect(body.field):is_nil()` holds.
            methods.add_method("json", |lua, this, ()| {
                let value: serde_json::Value = serde_json::from_str(&this.body).map_err(|e| {
                    mlua::Error::RuntimeError(format!("response body is not JSON: {e}"))
                })?;
                let opts = mlua::SerializeOptions::new()
                    .serialize_none_to_null(false)
                    .serialize_unit_to_null(false);
                lua.to_value_with(&value, opts)
            });
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let http = lua.create_table()?;
        http.set("get", method_fn(lua, reqwest::Method::GET)?)?;
        http.set("post", method_fn(lua, reqwest::Method::POST)?)?;
        http.set("put", method_fn(lua, reqwest::Method::PUT)?)?;
        http.set("patch", method_fn(lua, reqwest::Method::PATCH)?)?;
        http.set("delete", method_fn(lua, reqwest::Method::DELETE)?)?;
        http.set("head", method_fn(lua, reqwest::Method::HEAD)?)?;
        http.set("options", method_fn(lua, reqwest::Method::OPTIONS)?)?;
        http.set("wait_for", wait_for_fn(lua)?)?;
        // http.client{ base_url, headers?, timeout? } → a reusable REST client that prefixes base_url
        // and merges default headers (per-call headers/timeout override).
        http.set("client", client_fn(lua)?)?;
        // http.mock(ctx, opts?) → the `mock` facet: a real HTTP server, in-process, that you stub and
        // then assert on. `client` attaches to a real one, `mock` provisions a fake one.
        #[cfg(feature = "mock")]
        http.set("mock", super::mock::mock_fn(lua)?)?;
        Ok(http)
    }

    /// A reusable REST client bound to a base URL and default headers — the ergonomic path for a suite
    /// that hits one service many times (base URL + auth declared once). Methods mirror the free
    /// functions: `client:get/post/put/patch/delete/head/options(path, opts)` and `client:wait_for`.
    struct HttpClient {
        base_url: String,
        headers: Vec<(String, String)>,
        timeout: Option<Duration>,
    }

    impl UserData for HttpClient {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            client_method(methods, "get", reqwest::Method::GET);
            client_method(methods, "post", reqwest::Method::POST);
            client_method(methods, "put", reqwest::Method::PUT);
            client_method(methods, "patch", reqwest::Method::PATCH);
            client_method(methods, "delete", reqwest::Method::DELETE);
            client_method(methods, "head", reqwest::Method::HEAD);
            client_method(methods, "options", reqwest::Method::OPTIONS);
            methods.add_async_method(
                "wait_for",
                |lua, this, (path, opts): (String, Option<Table>)| {
                    let url = join_url(&this.base_url, &path);
                    let base_headers = this.headers.clone();
                    let params = wait_params(&opts);
                    async move {
                        let (expected, timeout, every) = params?;
                        let deadline = Instant::now() + timeout;
                        loop {
                            let prepared = Prepared {
                                method: reqwest::Method::GET,
                                url: url.clone(),
                                headers: base_headers.clone(),
                                body: None,
                                timeout: Some(every),
                            };
                            if let Ok(resp) = send(prepared).await {
                                if resp.status == expected {
                                    return lua.create_userdata(resp);
                                }
                            }
                            if Instant::now() >= deadline {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "http client wait_for timed out after {timeout:?} waiting for {expected} at {url}"
                                )));
                            }
                            tokio::time::sleep(every).await;
                        }
                    }
                },
            );
        }
    }

    fn client_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_function(|lua, opts: Table| {
            let base_url = opts.get::<Option<String>>("base_url")?.ok_or_else(|| {
                mlua::Error::RuntimeError("http.client requires a `base_url`".into())
            })?;
            let mut headers = Vec::new();
            if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
                for pair in hdrs.pairs::<String, String>() {
                    let (k, v) = pair?;
                    headers.push((k, v));
                }
            }
            let timeout = opts
                .get::<Option<String>>("timeout")?
                .and_then(|s| parse_duration(&s));
            lua.create_userdata(HttpClient {
                base_url,
                headers,
                timeout,
            })
        })
    }

    fn client_method<M: UserDataMethods<HttpClient>>(
        methods: &mut M,
        name: &'static str,
        method: reqwest::Method,
    ) {
        methods.add_async_method(
            name,
            move |lua, this, (path, opts): (String, Option<Table>)| {
                let url = join_url(&this.base_url, &path);
                let prepared = build_prepared(
                    &lua,
                    method.clone(),
                    url,
                    this.headers.clone(),
                    this.timeout,
                    opts,
                );
                async move {
                    let resp = send(prepared?).await?;
                    lua.create_userdata(resp)
                }
            },
        );
    }

    /// Join a client `base_url` with a per-call `path`. An absolute `path` (starting with a scheme)
    /// is used verbatim; otherwise exactly one `/` separates them.
    fn join_url(base: &str, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_string();
        }
        if path.is_empty() {
            return base.to_string();
        }
        let b = base.strip_suffix('/').unwrap_or(base);
        let p = path.strip_prefix('/').unwrap_or(path);
        format!("{b}/{p}")
    }

    /// An owned, Lua-free request spec, prepared synchronously so nothing borrows Lua across the
    /// await.
    struct Prepared {
        method: reqwest::Method,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        timeout: Option<Duration>,
    }

    fn method_fn(lua: &Lua, method: reqwest::Method) -> mlua::Result<Function> {
        lua.create_async_function(move |lua, (url, opts): (String, Option<Table>)| {
            let prepared = build_prepared(&lua, method.clone(), url, Vec::new(), None, opts);
            async move {
                let resp = send(prepared?).await?;
                lua.create_userdata(resp)
            }
        })
    }

    /// Build an owned request spec from `opts`, layered over optional defaults (a client's base
    /// headers/timeout). Per-call `headers` override defaults by name; `json`/`body`/`timeout` in
    /// `opts` win. Synchronous, so nothing borrows Lua across the await.
    fn build_prepared(
        lua: &Lua,
        method: reqwest::Method,
        url: String,
        mut headers: Vec<(String, String)>,
        mut timeout: Option<Duration>,
        opts: Option<Table>,
    ) -> mlua::Result<Prepared> {
        let mut body = None;
        if let Some(opts) = opts {
            if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
                for pair in hdrs.pairs::<String, String>() {
                    let (k, v) = pair?;
                    upsert_header(&mut headers, k, v);
                }
            }
            if let Some(json) = opts.get::<Option<Value>>("json")? {
                let value: serde_json::Value = lua.from_value(json)?;
                let encoded = serde_json::to_vec(&value).map_err(|e| {
                    mlua::Error::RuntimeError(format!("http: encoding json body: {e}"))
                })?;
                upsert_header(
                    &mut headers,
                    "content-type".into(),
                    "application/json".into(),
                );
                body = Some(encoded);
            } else if let Some(raw) = opts.get::<Option<String>>("body")? {
                body = Some(raw.into_bytes());
            }
            if let Some(t) = opts
                .get::<Option<String>>("timeout")?
                .and_then(|s| parse_duration(&s))
            {
                timeout = Some(t);
            }
        }
        Ok(Prepared {
            method,
            url,
            headers,
            body,
            timeout,
        })
    }

    /// Insert or replace a header by case-insensitive name (so a per-call header overrides a client
    /// default rather than sending both).
    fn upsert_header(headers: &mut Vec<(String, String)>, key: String, value: String) {
        match headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&key))
        {
            Some(existing) => existing.1 = value,
            None => headers.push((key, value)),
        }
    }

    async fn send(prepared: Prepared) -> mlua::Result<HttpResponse> {
        let client = reqwest::Client::new();
        let mut req = client.request(prepared.method, &prepared.url);
        for (k, v) in prepared.headers {
            req = req.header(k, v);
        }
        if let Some(body) = prepared.body {
            req = req.body(body);
        }
        if let Some(timeout) = prepared.timeout {
            req = req.timeout(timeout);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| mlua::Error::RuntimeError(format!("http request failed: {e}")))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
            .collect();
        let body = resp
            .text()
            .await
            .map_err(|e| mlua::Error::RuntimeError(format!("reading http response body: {e}")))?;
        Ok(HttpResponse {
            status,
            body,
            headers,
        })
    }

    /// `http.wait_for(url, { status = 200, timeout = "30s", every = "500ms" })` — poll GET until the
    /// endpoint returns the expected status or the deadline elapses. The boot-then-probe primitive.
    fn wait_for_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_async_function(|lua, (url, opts): (String, Option<Table>)| {
            let params = wait_params(&opts);
            async move {
                let (expected, timeout, every) = params?;
                let deadline = Instant::now() + timeout;
                loop {
                    let prepared = Prepared {
                        method: reqwest::Method::GET,
                        url: url.clone(),
                        headers: Vec::new(),
                        body: None,
                        timeout: Some(every),
                    };
                    if let Ok(resp) = send(prepared).await {
                        if resp.status == expected {
                            return lua.create_userdata(resp);
                        }
                    }
                    if Instant::now() >= deadline {
                        return Err(mlua::Error::RuntimeError(format!(
                            "http.wait_for timed out after {timeout:?} waiting for {expected} at {url}"
                        )));
                    }
                    tokio::time::sleep(every).await;
                }
            }
        })
    }

    fn wait_params(opts: &Option<Table>) -> mlua::Result<(u16, Duration, Duration)> {
        let mut status = 200;
        let mut timeout = Duration::from_secs(30);
        let mut every = Duration::from_millis(500);
        if let Some(opts) = opts {
            if let Some(s) = opts.get::<Option<u16>>("status")? {
                status = s;
            }
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
        }
        Ok((status, timeout, every))
    }
}

// ---------------------------------------------------------------------------------------------
// mock — the `mock` facet: an in-process stub/record server (`http.mock`)
// ---------------------------------------------------------------------------------------------

/// `http.mock` — the fourth facet, alongside `client` (attach to a real one), `container`
/// (provision a real one), and `wait_for` (probe one). It provisions a *fake* one: a real HTTP
/// server, in this process, that you stub, drive, and then assert on.
///
/// **It is not for the dependency you can run.** Prova's whole containerized-topology arc exists so
/// a test can drive the real thing; a mock earns its place on the boundary you cannot own (a partner
/// API), the behavior the real thing will not produce on demand (a 5xx, a timeout), and — the one
/// with no substitute — the *interaction itself*: a real dependency answers "did it work", never
/// "did we call it exactly once with the right idempotency key". See `docs/plans/mocks.md`.
///
/// **Handlers are Lua, and that is the point.** A stub's reply may be a table (terse) or a function
/// (general). The function runs on this very Lua state while the test coroutine that drove the SUT
/// is suspended — which is only possible because the engine is async to the ground (`engine.rs`:
/// bodies are `call_async`'d futures in a `FuturesUnordered`) and because `block_on_local` polls a
/// `LocalSet` alongside them. That is why there is no response-templating mini-language here: the
/// thing WireMock invented Handlebars to approximate is just a Lua closure.
///
/// **Readiness is a contract, as with `docker.run`'s `wait`.** The listener is bound *synchronously*
/// before `http.mock` returns, so the first request cannot race the bind and no caller needs a
/// `prova.retry`. In-process is what buys that — there is no daemon in the middle to lie about it.
#[cfg(feature = "mock")]
mod mock {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::time::Duration;

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use mlua::{
        Function, Lua, LuaSerdeExt, ObjectLike, Table, UserData, UserDataFields, UserDataMethods,
        Value,
    };

    use crate::model::parse_duration;

    /// A resolved response — what a `:reply{…}` table parsed to, or what a handler's returned table
    /// parsed to. One type for both paths, so a handler cannot express a response a declarative stub
    /// can't (and vice versa).
    #[derive(Clone)]
    struct ReplySpec {
        status: u16,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
        delay: Option<Duration>,
    }

    impl ReplySpec {
        fn plain(status: u16, msg: &str) -> Self {
            ReplySpec {
                status,
                body: msg.as_bytes().to_vec(),
                headers: vec![("content-type".into(), "text/plain".into())],
                delay: None,
            }
        }
    }

    #[derive(Clone)]
    enum Reply {
        /// `m:on{…}` was called but `:reply(…)` never was. A silent 200 would make a forgotten reply
        /// look like a passing test, so this answers 501 and records why.
        Unset,
        Data(ReplySpec),
        Handler(Function),
    }

    struct Stub {
        method: Option<String>,
        path: Option<String>,
        path_matches: Option<String>,
        route: Option<Vec<Seg>>,
        reply: Reply,
    }

    /// One segment of a compiled `route`.
    ///
    /// **Why `route` is its own key rather than an extension of `path`.** A literal colon is legal in
    /// a URL path and real APIs use it — Google's custom methods are spelled `/v1/models/x:predict`.
    /// Quietly reinterpreting `path` would break those. So exact-match keeps its meaning and
    /// templating gets a name that says so: `path` (exact) · `path_matches` (Lua pattern) · `route`
    /// (`:name` captures). Which one is in play is never ambiguous.
    #[derive(Clone)]
    enum Seg {
        Lit(String),
        Param(String),
    }

    fn compile_route(spec: &str) -> Vec<Seg> {
        spec.split('/')
            .map(|seg| match seg.strip_prefix(':') {
                Some(name) => Seg::Param(name.to_string()),
                None => Seg::Lit(seg.to_string()),
            })
            .collect()
    }

    /// Match a path against a compiled route, capturing params. Segment-wise, so a `:id` can never
    /// swallow a `/` — which is the default failure of the hand-rolled `(.+)$` this replaces.
    fn match_route(route: &[Seg], path: &str) -> Option<Vec<(String, String)>> {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != route.len() {
            return None;
        }
        let mut params = Vec::new();
        for (seg, part) in route.iter().zip(parts.iter()) {
            match seg {
                Seg::Lit(l) => {
                    if l != part {
                        return None;
                    }
                }
                Seg::Param(name) => {
                    if part.is_empty() {
                        return None;
                    }
                    params.push((name.clone(), (*part).to_string()));
                }
            }
        }
        Some(params)
    }

    /// The request as both the handler and the journal see it — deliberately the *same shape*, so
    /// `req.path` in a handler and `m:received()[1].path` in an assertion are the same field.
    struct RequestData {
        method: String,
        path: String,
        query: Vec<(String, String)>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    struct Recorded {
        req: RequestData,
        params: Vec<(String, String)>,
        status: u16,
        matched: bool,
        /// Who composed the answer: "stub" | "passthrough" | "replay" | "unmatched". `matched` stays
        /// narrowly "a stub matched", so a forwarded request reads as matched=false, source=passthrough.
        source: &'static str,
        error: Option<String>,
    }

    // -- cassettes ------------------------------------------------------------------------------

    /// A recorded exchange. Request headers are kept (they are often the thing under test — an
    /// idempotency key, a tenant id) but redacted; see `REDACTED_HEADERS`.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Cassette {
        version: u32,
        entries: Vec<Entry>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Entry {
        request: CassetteRequest,
        response: CassetteResponse,
    }

    /// `BTreeMap` so a cassette is byte-stable across runs: an unordered map would produce a
    /// different file every record and turn every re-record into an unreadable diff.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct CassetteRequest {
        method: String,
        path: String,
        query: BTreeMap<String, String>,
        headers: BTreeMap<String, String>,
        body: String,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct CassetteResponse {
        status: u16,
        headers: BTreeMap<String, String>,
        body: String,
    }

    /// Recording real traffic writes real traffic to a file someone will commit. These are redacted
    /// by default — a cassette carrying a live bearer token is a security incident, not a bug. This
    /// is a floor, not a guarantee: a bespoke auth header needs `redact = { … }`, and a cassette is
    /// real traffic that deserves a read before it is committed.
    const REDACTED_HEADERS: &[&str] = &[
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
        "x-api-key",
        "api-key",
        "x-auth-token",
    ];

    const REDACTION: &str = "REDACTED";

    /// Hop-by-hop headers, which describe *this* connection and must not be copied onto another one.
    /// Forwarding `content-length`/`transfer-encoding` in particular makes the upstream describe a
    /// body we then re-frame ourselves — a corrupt response that looks like a mock bug.
    const HOP_BY_HOP: &[&str] = &[
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        "keep-alive",
        "upgrade",
        "proxy-connection",
        "te",
        "trailer",
    ];

    fn redact_into(
        headers: &[(String, String)],
        extra: &[String],
        out: &mut BTreeMap<String, String>,
    ) {
        for (k, v) in headers {
            let redacted = REDACTED_HEADERS.contains(&k.as_str())
                || extra.iter().any(|e| e.eq_ignore_ascii_case(k));
            out.insert(
                k.clone(),
                if redacted {
                    REDACTION.to_string()
                } else {
                    v.clone()
                },
            );
        }
    }

    /// The replay key: method + path + query. Request *headers* are deliberately excluded — matching
    /// on them would make a cassette break on a rotated token or a changed date, which is drift the
    /// suite should not be reporting.
    fn replay_key(method: &str, path: &str, query: &BTreeMap<String, String>) -> String {
        let q: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{} {}?{}", method.to_ascii_uppercase(), path, q.join("&"))
    }

    struct Replay {
        entries: Vec<Entry>,
        consumed: Vec<bool>,
    }

    impl Replay {
        fn load(path: &str) -> mlua::Result<Self> {
            let text = std::fs::read_to_string(path).map_err(|e| {
                mlua::Error::RuntimeError(format!("http.mock: reading cassette {path:?}: {e}"))
            })?;
            let c: Cassette = serde_json::from_str(&text).map_err(|e| {
                mlua::Error::RuntimeError(format!("http.mock: parsing cassette {path:?}: {e}"))
            })?;
            let n = c.entries.len();
            Ok(Replay {
                entries: c.entries,
                consumed: vec![false; n],
            })
        }

        /// First *unconsumed* entry for this key. Consuming means repeated identical calls replay in
        /// recorded order (create → read-back reproduces instead of collapsing onto one answer),
        /// while different endpoints stay order-independent — a SUT that interleaves two calls is
        /// not doing anything wrong.
        fn take(&mut self, key: &str) -> Option<&CassetteResponse> {
            for (i, e) in self.entries.iter().enumerate() {
                if self.consumed[i] {
                    continue;
                }
                if replay_key(&e.request.method, &e.request.path, &e.request.query) == key {
                    self.consumed[i] = true;
                    return Some(&e.response);
                }
            }
            None
        }
    }

    #[derive(Default)]
    struct MockState {
        stubs: Vec<Stub>,
        journal: Vec<Recorded>,
        /// The dial. A proxy is a mock whose unmatched requests forward instead of 404 — one option,
        /// not a second concept, so stubs/journal/grammar are untouched by any of this.
        passthrough: Option<String>,
        record: Option<String>,
        replay: Option<Replay>,
        redact: Vec<String>,
        recorded: Vec<Entry>,
        /// Errors from *our own* stubs — a handler that raised, returned the wrong shape, or whose
        /// reply would not parse. Tracked apart from the journal's `error` field, which also covers
        /// a dead upstream and a replay miss: those are the *dependency* misbehaving (a 502 is a
        /// true report), whereas these are prova-side bugs wearing the dependency's clothes.
        handler_errors: Vec<String>,
        /// Opt out of strictness, for a test whose subject *is* the error path.
        allow_handler_errors: bool,
    }

    /// `Rc`/`RefCell` rather than `Arc`/`Mutex` on purpose: every task that touches this is
    /// `spawn_local`'d onto the same thread as the Lua state, so a cross-thread lock would be
    /// ceremony around a contention that cannot happen.
    type Shared = Rc<RefCell<MockState>>;

    struct MockServer {
        url: String,
        host: String,
        port: u16,
        /// The DNS name a container/VM/pod reaches this host-bound mock at, when `network` was
        /// requested — `host.docker.internal` by default (the Docker substrate's name for the host),
        /// overridable for another substrate. `None` → loopback-only, no cross-substrate vantage.
        network_host: Option<String>,
        state: Shared,
        shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    struct StubHandle {
        state: Shared,
        idx: usize,
    }

    /// `http.mock(ctx, opts?)` → a managed mock server.
    pub(crate) fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
            let server = start(lua, opts.as_ref())?;
            let ud = lua.create_userdata(server)?;
            // Tie the server's life to the caller's scope, exactly as a container's is. Going
            // through `ctx:manage` rather than reimplementing teardown means a mock is reaped by
            // the same LIFO machinery, in the same order, as every other resource — including under
            // `prova up`, where the scope is held until a signal rather than ending with a test.
            match ctx {
                Value::UserData(c) => {
                    let _: Value = c.call_method("manage", &ud)?;
                }
                Value::Nil => {
                    return Err(mlua::Error::RuntimeError(
                        "http.mock(ctx): pass the test or fixture context (`t` / `ctx`) so the \
                         server is torn down with the scope"
                            .into(),
                    ))
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "http.mock(ctx): expected the test or fixture context, got a {}",
                        other.type_name()
                    )))
                }
            }
            Ok(ud)
        })
    }

    /// Bind synchronously (so the port is known and the socket is accepting before we return), then
    /// `spawn_local` the accept loop onto the engine's `LocalSet`.
    fn start(lua: &Lua, opts: Option<&Table>) -> mlua::Result<MockServer> {
        let mut init = MockState::default();
        // A mock is a *host* process; a container reaches it not by a DNS alias (it is not on the
        // docker network) but at the host gateway. `network` opts into that: it binds all interfaces
        // (a real LAN exposure, hence off by default) and exposes a `.network` vantage the SUT wires
        // in. `true` → `host.docker.internal`; a string overrides the host name for another substrate.
        let mut network_host: Option<String> = None;
        if let Some(o) = opts {
            match o.get::<Option<Value>>("network")? {
                Some(Value::Boolean(true)) => {
                    network_host = Some("host.docker.internal".to_string())
                }
                Some(Value::String(name)) => {
                    network_host = Some(name.to_string_lossy().to_string())
                }
                Some(Value::Boolean(false)) | None | Some(Value::Nil) => {}
                Some(other) => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "http.mock: `network` must be true or a host name, got a {}",
                        other.type_name()
                    )))
                }
            }
            init.passthrough = o.get::<Option<String>>("passthrough")?;
            init.record = o.get::<Option<String>>("record")?;
            init.allow_handler_errors = o
                .get::<Option<bool>>("allow_handler_errors")?
                .unwrap_or(false);
            let replay_path = o.get::<Option<String>>("replay")?;
            if let Some(t) = o.get::<Option<Table>>("redact")? {
                for h in t.sequence_values::<String>() {
                    init.redact.push(h?.to_ascii_lowercase());
                }
            }
            // Invalid states, rejected at the call site rather than surfacing as a confusing 404 or
            // an empty cassette three tests later.
            if init.passthrough.is_some() && replay_path.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "http.mock: `passthrough` and `replay` are mutually exclusive — one forwards to \
                     a real dependency, the other answers from a recording of one"
                        .into(),
                ));
            }
            if init.record.is_some() && init.passthrough.is_none() {
                return Err(mlua::Error::RuntimeError(
                    "http.mock: `record` needs `passthrough` — a cassette records what a real \
                     dependency answered, and there is nothing to record without one"
                        .into(),
                ));
            }
            if let Some(p) = replay_path {
                init.replay = Some(Replay::load(&p)?);
            }
        }

        // Loopback unless a cross-substrate vantage was asked for. Binding all interfaces is the
        // security-relevant bit, so it is gated on the same explicit `network` request.
        let bind_ip = if network_host.is_some() {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        let std_listener = std::net::TcpListener::bind((bind_ip, 0))
            .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: bind: {e}")))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: set_nonblocking: {e}")))?;
        let port = std_listener
            .local_addr()
            .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: local_addr: {e}")))?
            .port();
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: from_std: {e}")))?;

        let state: Shared = Rc::new(RefCell::new(init));
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        let accept_state = state.clone();
        let accept_lua = lua.clone();
        // `spawn_local`, never `tokio::spawn`: this task holds a `Lua` handle (to call handlers),
        // and mlua handles are `!Send`. See `engine::block_on_local` for why a `LocalSet` exists.
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else { break };
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let conn_state = accept_state.clone();
                        let conn_lua = accept_lua.clone();
                        tokio::task::spawn_local(async move {
                            let svc = service_fn(move |req: Request<Incoming>| {
                                let s = conn_state.clone();
                                let l = conn_lua.clone();
                                async move { handle(l, s, req).await }
                            });
                            // http1 specifically: it puts no `Send` bound on the service or its
                            // future, which is what lets a Lua handler live inside one. axum and
                            // anything tower-shaped bound it `Send` and cannot express this.
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, svc)
                                .await;
                        });
                    }
                }
            }
        });

        Ok(MockServer {
            // `url`/`host` remain loopback: they are how *this* process (the test) probes the mock,
            // and 0.0.0.0 includes loopback. The cross-substrate address lives on `.network`.
            url: format!("http://127.0.0.1:{port}"),
            host: "127.0.0.1".to_string(),
            port,
            network_host,
            state,
            shutdown: RefCell::new(Some(tx)),
        })
    }

    async fn handle(
        lua: Lua,
        state: Shared,
        req: Request<Incoming>,
    ) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
        let rd = read_request(req).await;

        // Resolve the match and clone the reply out before doing anything that can re-enter Lua: a
        // handler may legitimately call `m:on{…}` or `m:received()`, which borrows this same
        // RefCell. Holding a borrow across an await into Lua would panic at runtime.
        let hit = match find_match(&lua, &state, &rd) {
            Ok(h) => h,
            Err(e) => {
                return Ok(respond(
                    &state,
                    rd,
                    Vec::new(),
                    ReplySpec::plain(500, &format!("mock: matching failed: {e}")),
                    false,
                    "stub",
                    Some(e.to_string()),
                ))
            }
        };
        let params: Vec<(String, String)> =
            hit.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
        let reply = hit
            .as_ref()
            .map(|(i, _)| state.borrow().stubs[*i].reply.clone());

        // A stub always wins over the dial. That is what makes *partial* mocking work: stub the one
        // endpoint you need to control, let everything else reach the real service.
        let (spec, source, error) = match reply {
            Some(Reply::Unset) => (
                ReplySpec::plain(501, "prova http.mock: stub matched but has no :reply(…)"),
                "stub",
                Some("stub matched but has no :reply(…)".to_string()),
            ),
            Some(Reply::Data(d)) => (d, "stub", None),
            Some(Reply::Handler(f)) => {
                let (s, e) = run_handler(&lua, f, &rd, &params).await;
                (s, "stub", e)
            }
            None => unmatched(&state, &rd).await,
        };

        if let Some(d) = spec.delay {
            tokio::time::sleep(d).await;
        }
        let matched = hit.is_some();
        Ok(respond(&state, rd, params, spec, matched, source, error))
    }

    /// No stub matched: consult the dial. Replay answers from a recording; passthrough forwards to
    /// the real dependency; otherwise it is a 404, exactly as in Phase A.
    async fn unmatched(
        state: &Shared,
        rd: &RequestData,
    ) -> (ReplySpec, &'static str, Option<String>) {
        let (has_replay, passthrough) = {
            let s = state.borrow();
            (s.replay.is_some(), s.passthrough.clone())
        };

        if has_replay {
            let query: BTreeMap<String, String> = rd.query.iter().cloned().collect();
            let key = replay_key(&rd.method, &rd.path, &query);
            let hit = {
                let mut s = state.borrow_mut();
                s.replay
                    .as_mut()
                    .and_then(|r| r.take(&key))
                    .map(|resp| ReplySpec {
                        status: resp.status,
                        body: resp.body.clone().into_bytes(),
                        headers: resp
                            .headers
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        delay: None,
                    })
            };
            return match hit {
                Some(spec) => (spec, "replay", None),
                // Strict on purpose. Inventing an answer for a call the cassette never recorded
                // would let the SUT change behavior without the suite noticing — the exact failure
                // a cassette exists to catch.
                None => {
                    let msg = format!(
                        "prova http.mock: cassette has no unconsumed entry for {key} — re-record it \
                         if the system under test legitimately changed"
                    );
                    (ReplySpec::plain(404, &msg), "replay", Some(msg))
                }
            };
        }

        if let Some(base) = passthrough {
            return match forward(&base, rd).await {
                Ok(spec) => {
                    record_exchange(state, rd, &spec);
                    (spec, "passthrough", None)
                }
                // 502 is the honest status: *we* are a gateway and the upstream did not answer.
                // Reporting the mock's own failure as a 500 would blame the SUT for our plumbing.
                Err(e) => (
                    ReplySpec::plain(502, &format!("prova http.mock: upstream {base}: {e}")),
                    "passthrough",
                    Some(e),
                ),
            };
        }

        (
            ReplySpec::plain(404, "prova http.mock: no matching stub"),
            "unmatched",
            None,
        )
    }

    /// Forward one request to the real dependency, verbatim but for the hop-by-hop headers.
    async fn forward(base: &str, rd: &RequestData) -> Result<ReplySpec, String> {
        let mut url = format!("{}{}", base.trim_end_matches('/'), rd.path);
        if !rd.query.is_empty() {
            let q: Vec<String> = rd
                .query
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>(),
                        form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()
                    )
                })
                .collect();
            url.push('?');
            url.push_str(&q.join("&"));
        }
        let method = reqwest::Method::from_bytes(rd.method.as_bytes())
            .map_err(|e| format!("bad method {:?}: {e}", rd.method))?;
        let mut req = reqwest::Client::new().request(method, &url);
        for (k, v) in &rd.headers {
            if HOP_BY_HOP.contains(&k.as_str()) {
                continue;
            }
            req = req.header(k.as_str(), v.as_str());
        }
        if !rd.body.is_empty() {
            req = req.body(rd.body.clone());
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter(|(k, _)| !HOP_BY_HOP.contains(&k.as_str()))
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
        Ok(ReplySpec {
            status,
            body,
            headers,
            delay: None,
        })
    }

    /// Append a forwarded exchange to the pending cassette. Only *forwarded* traffic is recorded: a
    /// cassette is a recording of the real dependency, and recording our own stubs back to ourselves
    /// would make replay assert that the mock agrees with the mock.
    fn record_exchange(state: &Shared, rd: &RequestData, spec: &ReplySpec) {
        let mut s = state.borrow_mut();
        if s.record.is_none() {
            return;
        }
        let mut req_headers = BTreeMap::new();
        redact_into(&rd.headers, &s.redact, &mut req_headers);
        let mut resp_headers = BTreeMap::new();
        redact_into(&spec.headers, &s.redact, &mut resp_headers);
        s.recorded.push(Entry {
            request: CassetteRequest {
                method: rd.method.clone(),
                path: rd.path.clone(),
                query: rd.query.iter().cloned().collect(),
                headers: req_headers,
                body: String::from_utf8_lossy(&rd.body).to_string(),
            },
            response: CassetteResponse {
                status: spec.status,
                headers: resp_headers,
                body: String::from_utf8_lossy(&spec.body).to_string(),
            },
        });
    }

    /// Call a Lua reply handler. An error here must not be silent: it answers 500 *and* lands in the
    /// journal, so a broken handler is visible to an assertion rather than looking like the
    /// dependency legitimately failed.
    async fn run_handler(
        lua: &Lua,
        f: Function,
        rd: &RequestData,
        params: &[(String, String)],
    ) -> (ReplySpec, Option<String>) {
        let req_tbl = match req_to_lua(lua, rd, params) {
            Ok(t) => t,
            Err(e) => {
                return (
                    ReplySpec::plain(500, "mock: handler input"),
                    Some(e.to_string()),
                )
            }
        };
        match f.call_async::<Value>(req_tbl).await {
            Ok(Value::Table(t)) => match parse_reply(lua, &t) {
                Ok(s) => (s, None),
                Err(e) => (
                    ReplySpec::plain(500, &format!("mock: handler reply: {e}")),
                    Some(e.to_string()),
                ),
            },
            Ok(other) => {
                let msg = format!(
                    "mock: handler must return a response table, returned a {}",
                    other.type_name()
                );
                (ReplySpec::plain(500, &msg), Some(msg))
            }
            Err(e) => (
                ReplySpec::plain(500, &format!("mock: handler raised: {e}")),
                Some(e.to_string()),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn respond(
        state: &Shared,
        req: RequestData,
        params: Vec<(String, String)>,
        spec: ReplySpec,
        matched: bool,
        source: &'static str,
        error: Option<String>,
    ) -> Response<Full<Bytes>> {
        // A stub-sourced error is *our* bug, not the dependency's: track it so `stop()` can fail the
        // owning scope. Without this a SUT with a retry or a fallback swallows the 500 and the suite
        // goes green over a broken handler, blaming the dependency for flakiness.
        if source == "stub" {
            if let Some(e) = &error {
                state.borrow_mut().handler_errors.push(e.clone());
            }
        }
        // Record *every* request, matched or not. An unmatched call is usually the most interesting
        // thing a mock can tell you — it is the SUT doing something you did not predict.
        state.borrow_mut().journal.push(Recorded {
            req,
            params,
            status: spec.status,
            matched,
            source,
            error,
        });

        let mut builder = Response::builder().status(spec.status);
        for (k, v) in &spec.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        builder
            .body(Full::new(Bytes::from(spec.body)))
            .unwrap_or_else(|e| {
                Response::builder()
                    .status(500)
                    .body(Full::new(Bytes::from(format!("mock: bad response: {e}"))))
                    .expect("500 with a plain body is always constructible")
            })
    }

    async fn read_request(req: Request<Incoming>) -> RequestData {
        let method = req.method().as_str().to_string();
        let uri = req.uri().clone();
        let path = uri.path().to_string();
        let query = uri
            .query()
            .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
            .unwrap_or_default();
        let headers = req
            .headers()
            .iter()
            .map(|(k, v)| {
                // Lowercase: HTTP header names are case-insensitive, so a journal that preserved the
                // sender's casing would make `r.headers["X-Idempotency-Key"]` work or not depending
                // on which client wrote the request. One spelling, always.
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = req
            .into_body()
            .collect()
            .await
            .map(|c| c.to_bytes().to_vec())
            .unwrap_or_default();
        RequestData {
            method,
            path,
            query,
            headers,
            body,
        }
    }

    /// First match wins — insertion order. A later, more specific stub does not override an earlier
    /// general one, because "most specific wins" needs a specificity ranking, and every ranking is a
    /// rule you have to know before you can read a test.
    type Candidate = (usize, Option<String>, Option<Vec<Seg>>);
    /// The matching stub's index plus the params its `route` captured.
    type Hit = (usize, Vec<(String, String)>);

    fn find_match(lua: &Lua, state: &Shared, rd: &RequestData) -> mlua::Result<Option<Hit>> {
        // Collect the patterns first: matching calls back into Lua (`string.match`), and Lua could
        // re-enter this RefCell.
        let candidates: Vec<Candidate> = {
            let s = state.borrow();
            s.stubs
                .iter()
                .enumerate()
                .filter(|(_, stub)| {
                    stub.method
                        .as_ref()
                        .is_none_or(|m| rd.method.eq_ignore_ascii_case(m))
                        && stub.path.as_ref().is_none_or(|p| &rd.path == p)
                })
                .map(|(i, stub)| (i, stub.path_matches.clone(), stub.route.clone()))
                .collect()
        };
        for (i, pat, route) in candidates {
            if let Some(r) = route {
                match match_route(&r, &rd.path) {
                    Some(params) => return Ok(Some((i, params))),
                    None => continue,
                }
            }
            match pat {
                None => return Ok(Some((i, Vec::new()))),
                // Lua patterns, not regex — `path_matches` must mean exactly what `:matches(pat)`
                // means everywhere else in the assertion surface, so ask Lua rather than reimplement.
                Some(p) => {
                    let string: Table = lua.globals().get("string")?;
                    let matcher: Function = string.get("match")?;
                    let r: Value = matcher.call((rd.path.clone(), p))?;
                    if !matches!(r, Value::Nil) {
                        return Ok(Some((i, Vec::new())));
                    }
                }
            }
        }
        Ok(None)
    }

    fn req_to_lua(lua: &Lua, rd: &RequestData, params: &[(String, String)]) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("method", rd.method.clone())?;
        t.set("path", rd.path.clone())?;
        let q = lua.create_table()?;
        for (k, v) in &rd.query {
            q.set(k.clone(), v.clone())?;
        }
        t.set("query", q)?;
        let h = lua.create_table()?;
        for (k, v) in &rd.headers {
            h.set(k.clone(), v.clone())?;
        }
        t.set("headers", h)?;
        t.set("body", lua.create_string(&rd.body)?)?;
        let p = lua.create_table()?;
        for (k, v) in params {
            p.set(k.clone(), v.clone())?;
        }
        t.set("params", p)?;
        // `json` is a convenience, not a contract: nil when the body isn't JSON. Unlike the http
        // client's `res:json()` (which raises), a request body you didn't send isn't your bug — and
        // a handler wants to branch on shape, not defend against a raise.
        if let Ok(jv) = serde_json::from_slice::<serde_json::Value>(&rd.body) {
            t.set("json", lua.to_value(&jv)?)?;
        }
        Ok(t)
    }

    fn recorded_to_lua(lua: &Lua, r: &Recorded) -> mlua::Result<Table> {
        let t = req_to_lua(lua, &r.req, &r.params)?;
        t.set("status", r.status)?;
        t.set("matched", r.matched)?;
        t.set("source", r.source)?;
        if let Some(e) = &r.error {
            t.set("error", e.clone())?;
        }
        Ok(t)
    }

    fn parse_reply(lua: &Lua, t: &Table) -> mlua::Result<ReplySpec> {
        let status = t.get::<Option<u16>>("status")?.unwrap_or(200);
        if !(100..=599).contains(&status) {
            return Err(mlua::Error::RuntimeError(format!(
                "mock reply: status must be 100..599, got {status}"
            )));
        }

        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(h) = t.get::<Option<Table>>("headers")? {
            for pair in h.pairs::<String, String>() {
                let (k, v) = pair?;
                headers.push((k.to_ascii_lowercase(), v));
            }
        }

        let json = t.get::<Option<Value>>("json")?.filter(|v| !v.is_nil());
        let body_str = t.get::<Option<String>>("body")?;
        if json.is_some() && body_str.is_some() {
            return Err(mlua::Error::RuntimeError(
                "mock reply: has both `json` and `body` — a response has one body, not two".into(),
            ));
        }

        let body = match (json, body_str) {
            (Some(j), _) => {
                let jv: serde_json::Value = lua.from_value(j)?;
                let bytes = serde_json::to_vec(&jv).map_err(|e| {
                    mlua::Error::RuntimeError(format!("mock reply: encoding `json`: {e}"))
                })?;
                if !headers.iter().any(|(k, _)| k == "content-type") {
                    headers.push(("content-type".into(), "application/json".into()));
                }
                bytes
            }
            (None, Some(b)) => b.into_bytes(),
            (None, None) => Vec::new(),
        };

        let delay = match t.get::<Option<String>>("delay")? {
            Some(s) => Some(parse_duration(&s).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("mock reply: bad `delay` duration {s:?}"))
            })?),
            None => None,
        };

        Ok(ReplySpec {
            status,
            body,
            headers,
            delay,
        })
    }

    impl UserData for MockServer {
        fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
            // The grammar's fields, same as any resource: wire `m.url` into the SUT exactly the way
            // you wire a database's.
            fields.add_field_method_get("url", |_, this| Ok(this.url.clone()));
            fields.add_field_method_get("host", |_, this| Ok(this.host.clone()));
            fields.add_field_method_get("port", |_, this| Ok(this.port));
            // `.network` — the vantage a containerized/VM'd SUT wires in, present only when
            // `network` was requested. Mirrors a container resource's `.network`, but the address is
            // the host gateway rather than a DNS alias, because a mock is a host process.
            fields.add_field_method_get("network", |lua, this| {
                let Some(host) = &this.network_host else {
                    return Ok(Value::Nil);
                };
                let t = lua.create_table()?;
                t.set("url", format!("http://{host}:{}", this.port))?;
                t.set("host", host.clone())?;
                t.set("port", this.port)?;
                Ok(Value::Table(t))
            });
        }

        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // m:on{ method?, path?, path_matches? } → a stub handle to :reply on.
            methods.add_method("on", |lua, this, spec: Table| {
                let stub = Stub {
                    method: spec
                        .get::<Option<String>>("method")?
                        .map(|m| m.to_ascii_uppercase()),
                    path: spec.get::<Option<String>>("path")?,
                    path_matches: spec.get::<Option<String>>("path_matches")?,
                    route: spec
                        .get::<Option<String>>("route")?
                        .as_deref()
                        .map(compile_route),
                    reply: Reply::Unset,
                };
                let idx = {
                    let mut s = this.state.borrow_mut();
                    s.stubs.push(stub);
                    s.stubs.len() - 1
                };
                lua.create_userdata(StubHandle {
                    state: this.state.clone(),
                    idx,
                })
            });

            // m:received(filter?) → the journal, as plain Lua tables. Deliberately *data*, not a
            // `verify(count, pattern)` DSL: `t:expect` already asserts, and the matchers were never
            // stringly-typed. A new matcher only if the journal proves to need one.
            methods.add_method("received", |lua, this, filter: Option<Table>| {
                let (want_method, want_path) = match &filter {
                    Some(f) => (
                        f.get::<Option<String>>("method")?,
                        f.get::<Option<String>>("path")?,
                    ),
                    None => (None, None),
                };
                let out = lua.create_table()?;
                let s = this.state.borrow();
                let mut n = 0;
                for r in s.journal.iter() {
                    if let Some(m) = &want_method {
                        if !r.req.method.eq_ignore_ascii_case(m) {
                            continue;
                        }
                    }
                    if let Some(p) = &want_path {
                        if &r.req.path != p {
                            continue;
                        }
                    }
                    n += 1;
                    out.set(n, recorded_to_lua(lua, r)?)?;
                }
                Ok(out)
            });

            // `stop` is what `ctx:manage` calls; idempotent, so an explicit stop plus scope teardown
            // is not an error.
            // `stop` is what `ctx:manage` calls; idempotent, so an explicit stop plus scope teardown
            // is not an error. The cassette is written here rather than per-request so a suite that
            // fails mid-way still leaves a coherent file — teardown runs on failure too.
            //
            // Raising here is how a handler error reaches a report: a handler runs on a server task,
            // outside any test's stack, so there is nowhere for it to land at the time. `ctx:manage`
            // calls this at scope end and a raising teardown is its own reported leaf — so this needs
            // no mock-specific reporting path at all.
            methods.add_method("stop", |_, this, ()| {
                if let Some(tx) = this.shutdown.borrow_mut().take() {
                    let _ = tx.send(());
                    write_cassette(&this.state)?;
                }
                let errs = take_handler_errors(&this.state);
                if !errs.is_empty() {
                    return Err(handler_error_report("http.mock", &errs));
                }
                Ok(())
            });
        }
    }

    /// Drain the handler errors — so an explicit `m:stop()` followed by scope teardown reports once,
    /// not twice.
    fn take_handler_errors(state: &Shared) -> Vec<String> {
        let mut s = state.borrow_mut();
        if s.allow_handler_errors {
            s.handler_errors.clear();
            return Vec::new();
        }
        std::mem::take(&mut s.handler_errors)
    }

    pub(super) fn handler_error_report(ns: &str, errs: &[String]) -> mlua::Error {
        let n = errs.len();
        let plural = if n == 1 { "" } else { "s" };
        mlua::Error::RuntimeError(format!(
            "{ns}: {n} reply-handler error{plural} — the mock's own stub failed, so a green run here \
             would be reporting prova's bug as the dependency's. First: {}\n\
             If the error path is the subject of the test, pass `allow_handler_errors = true`.",
            errs[0]
        ))
    }

    fn write_cassette(state: &Shared) -> mlua::Result<()> {
        let mut s = state.borrow_mut();
        let Some(path) = s.record.clone() else {
            return Ok(());
        };
        let cassette = Cassette {
            version: 1,
            entries: std::mem::take(&mut s.recorded),
        };
        let text = serde_json::to_string_pretty(&cassette)
            .map_err(|e| mlua::Error::RuntimeError(format!("http.mock: encoding cassette: {e}")))?;
        std::fs::write(&path, text).map_err(|e| {
            mlua::Error::RuntimeError(format!("http.mock: writing cassette {path:?}: {e}"))
        })
    }

    impl UserData for StubHandle {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("reply", |lua, this, v: Value| {
                let reply = match v {
                    // The primitive. `topologies.md`: the convenience never removes it.
                    Value::Function(f) => Reply::Handler(f),
                    // The convenience — and the form `prova up` can serve with no test in scope, and
                    // that a cassette round-trips to.
                    Value::Table(t) => Reply::Data(parse_reply(lua, &t)?),
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "mock :reply expects a response table or a handler function, got a {}",
                            other.type_name()
                        )))
                    }
                };
                this.state.borrow_mut().stubs[this.idx].reply = reply;
                Ok(())
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// docker (testcontainers-style ephemeral dependencies, via the typed bollard daemon client)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "docker")]
pub(crate) mod docker {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use bollard::container::{
        Config, CreateContainerOptions, LogOutput, LogsOptions, NetworkingConfig,
        RemoveContainerOptions, StartContainerOptions,
    };
    use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
    use bollard::image::CreateImageOptions;
    use bollard::models::{EndpointSettings, HostConfig, PortBinding};
    use bollard::network::CreateNetworkOptions;
    use bollard::Docker;
    use futures::StreamExt;
    use mlua::{
        AnyUserData, Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value,
    };

    use crate::model::parse_duration;

    /// A running container from `docker.run` — same Lua surface as before, now backed by the typed
    /// bollard daemon client (structured errors, streamed logs/exec, no CLI parsing). `c.id`,
    /// `c:host_port(p)`, `c:endpoint(p)`, async `c:logs()`, `c:exec(cmd)`, `c:stop()`. `:stop`
    /// force-removes; a `Drop` backstop removes it if a test forgot to. Blessed pattern:
    /// `ctx:defer(function() c:stop() end)`.
    struct Container {
        client: Docker,
        id: String,
        ports: HashMap<u16, u16>, // container port -> mapped host port (best-effort cache)
        /// The ports `docker.run` was asked to publish. Kept so `host_port` can distinguish a
        /// mapping that is merely *late* (wait for it) from one that was never requested at all
        /// (fail immediately — no amount of waiting will conjure it).
        requested: Vec<u16>,
        /// The alias this container answers to on its user-defined network (from `docker.run`'s
        /// `alias`), if it joined one with an alias. Siblings resolve it via embedded DNS.
        alias: Option<String>,
        stopped: bool,
    }

    impl Drop for Container {
        fn drop(&mut self) {
            if !self.stopped {
                // Last-resort, fire-and-forget removal so a container never leaks even if cleanup
                // was skipped. bollard can't run in a sync Drop, so shell out for just this net.
                let _ = std::process::Command::new("docker")
                    .args(["rm", "-f", &self.id])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }

    impl UserData for Container {
        fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
            fields.add_field_method_get("id", |_, this| Ok(this.id.clone()));
        }
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // The network alias this container was created with (nil if it joined no network, or
            // joined one without an alias). Set at create time from `docker.run`'s `alias`.
            methods.add_method("network_alias", |_, this, ()| Ok(this.alias.clone()));
            methods.add_async_method("host_port", |_, this, port: u16| async move {
                resolved_host_port(&this, port).await
            });
            methods.add_async_method("endpoint", |_, this, port: u16| async move {
                resolved_host_port(&this, port)
                    .await
                    .map(|hp| format!("127.0.0.1:{hp}"))
            });
            methods.add_async_method("logs", |_, this, ()| {
                let client = this.client.clone();
                let id = this.id.clone();
                async move { container_logs(&client, &id).await }
            });
            // Low-level: run a shell command, return (exit_code, stdout, stderr) — no raising.
            methods.add_async_method("exec", |_, this, cmd: String| {
                let client = this.client.clone();
                let id = this.id.clone();
                async move {
                    container_exec(&client, &id, vec!["sh".into(), "-c".into(), cmd], None).await
                }
            });
            // Ergonomic: run a command (argv table = no shell/no quoting, or a string = `sh -c`),
            // optionally piping `opts.stdin`; raise on non-zero exit, return stdout. This is the
            // exec-CLI SDK entry point — a plugin drives a CLI in the container without hand-rolling
            // shell-quoting or `printf | …` piping (see docs/design/ecosystem.md).
            methods.add_async_method("run", |_, this, (cmd, opts): (Value, Option<Table>)| {
                let client = this.client.clone();
                let id = this.id.clone();
                let parsed = parse_run_cmd(cmd, opts);
                async move {
                    let (argv, stdin) = parsed?;
                    let (code, out, err) = container_exec(&client, &id, argv, stdin).await?;
                    if code != 0 {
                        let detail = if err.trim().is_empty() { &out } else { &err };
                        return Err(mlua::Error::RuntimeError(format!(
                            "container:run exited {code}: {}",
                            detail.trim()
                        )));
                    }
                    Ok(out)
                }
            });
            methods.add_async_method_mut("stop", |_, mut this, ()| {
                let client = this.client.clone();
                let id = this.id.clone();
                let already = this.stopped;
                this.stopped = true;
                async move {
                    if !already {
                        let _ = client
                            .remove_container(
                                &id,
                                Some(RemoveContainerOptions {
                                    force: true,
                                    ..Default::default()
                                }),
                            )
                            .await;
                    }
                    Ok(())
                }
            });
        }
    }

    /// A user-defined bridge network from `docker.network` — a handle with a `name` field and an
    /// async teardown (`stop`) that removes the network. Blessed pattern: `ctx:manage(net)`, which
    /// tears it down LIFO *after* its containers. A `Drop` backstop shells out to remove it if
    /// cleanup was skipped, so a network never leaks.
    struct Network {
        client: Docker,
        name: String,
        removed: bool,
    }

    impl Drop for Network {
        fn drop(&mut self) {
            if !self.removed {
                // Last-resort, fire-and-forget removal (bollard can't run in a sync Drop).
                let _ = std::process::Command::new("docker")
                    .args(["network", "rm", "-f", &self.name])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }

    impl UserData for Network {
        fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
            fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        }
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // Teardown (via `ctx:manage`): remove the network. Under LIFO teardown a container is
            // removed just before its network, but a container started with `--rm` may still be
            // detaching its endpoint when we get here — Docker then rejects the removal with "has
            // active endpoints". Retry briefly until the endpoints drain, then give up quietly (the
            // Drop backstop catches a genuine leak).
            methods.add_async_method_mut("stop", |_, mut this, ()| {
                let client = this.client.clone();
                let name = this.name.clone();
                let already = this.removed;
                this.removed = true;
                async move {
                    if !already {
                        let deadline = Instant::now() + Duration::from_secs(15);
                        loop {
                            match client.remove_network(&name).await {
                                Ok(()) => break,
                                Err(_) if Instant::now() < deadline => {
                                    tokio::time::sleep(Duration::from_millis(200)).await;
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    Ok(())
                }
            });
        }
    }

    /// A process-unique, human-recognizable network name: `prova-net-<pid>-<counter>`. Scripts
    /// can't reach a good entropy source, but Rust can — mirror how temp destinations are named
    /// (process id + a monotonic counter) so concurrent runs never collide.
    fn unique_network_name() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("prova-net-{}-{}", std::process::id(), n)
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let docker = lua.create_table()?;
        docker.set("run", run_fn(lua)?)?;
        docker.set("build", build_fn(lua)?)?;
        docker.set("network", network_fn(lua)?)?;
        // `docker.diagnostics()` — what the container runtime got wrong that prova papered over.
        // Process-wide and monotonic, so a caller reads it before and after and takes the delta.
        docker.set(
            "diagnostics",
            lua.create_function(|lua, ()| {
                let t = lua.create_table()?;
                t.set(
                    "port_bind_recoveries",
                    PORT_BIND_RECOVERIES.load(Ordering::Relaxed),
                )?;
                t.set(
                    "port_bind_failures",
                    PORT_BIND_FAILURES.load(Ordering::Relaxed),
                )?;
                Ok(t)
            })?,
        )?;
        Ok(docker)
    }

    fn run_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_async_function(|lua, opts: Table| {
            let spec = Spec::from_table(&opts);
            async move {
                let container = start(spec?).await?;
                lua.create_userdata(container)
            }
        })
    }

    /// What `docker.build` needs off the Lua opts table. `context` is the build-context directory;
    /// `dockerfile` is relative to it (Docker's own rule — `COPY` resolves against the context root,
    /// not the Dockerfile's directory), defaulting to `Dockerfile`.
    struct BuildSpec {
        context: String,
        dockerfile: String,
        tag: String,
        buildargs: Vec<(String, String)>,
        target: Option<String>,
        pull: bool,
        nocache: bool,
    }

    /// A default image tag derived from the context path — **stable across runs**, so a rebuild
    /// *replaces* the previous tag instead of leaking a dangling image every run, and the builder's
    /// layer cache hits. (Unique-per-run names are right for networks — cheap, and they must not
    /// collide; they are wrong for images, which are expensive and want reuse.)
    fn default_build_tag(context: &str) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in context.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("prova-build-{hash:x}:latest")
    }

    impl BuildSpec {
        fn from_table(t: &Table) -> mlua::Result<Self> {
            let context: String = t.get::<Option<String>>("context")?.ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "docker.build: `context` (a directory) is required".into(),
                )
            })?;
            if !std::path::Path::new(&context).is_dir() {
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.build: context `{context}` is not a directory"
                )));
            }
            let dockerfile = t
                .get::<Option<String>>("dockerfile")?
                .unwrap_or_else(|| "Dockerfile".to_string());
            // Fail here rather than handing the builder a path it rejects with a murkier message.
            if !std::path::Path::new(&context).join(&dockerfile).is_file() {
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.build: no dockerfile at `{dockerfile}` (relative to context `{context}`)"
                )));
            }
            let tag = t
                .get::<Option<String>>("tag")?
                .unwrap_or_else(|| default_build_tag(&context));

            let mut buildargs = Vec::new();
            if let Some(args) = t.get::<Option<Table>>("buildargs")? {
                for pair in args.pairs::<String, Value>() {
                    let (k, v) = pair?;
                    // Scalars coerce, so a numeric build arg stays a number on the Lua side.
                    let v = match v {
                        Value::String(s) => s.to_str()?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Boolean(b) => b.to_string(),
                        other => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "docker.build: buildarg `{k}` must be a scalar, got {}",
                                other.type_name()
                            )))
                        }
                    };
                    buildargs.push((k, v));
                }
            }

            Ok(BuildSpec {
                context,
                dockerfile,
                tag,
                buildargs,
                target: t.get::<Option<String>>("target")?,
                pull: t.get::<Option<bool>>("pull")?.unwrap_or(false),
                nocache: t.get::<Option<bool>>("nocache")?.unwrap_or(false),
            })
        }
    }

    /// `docker.build{ context, dockerfile?, tag?, buildargs?, target?, pull?, nocache? }` — build a
    /// local image from a Dockerfile and return its ref, ready for `docker.run{ image = … }`.
    ///
    /// This shells out to the `docker` CLI rather than driving bollard's build endpoint, for two
    /// substantive reasons:
    ///
    /// - **BuildKit.** The CLI gets it by default; bollard 0.18's classic builder does not (its
    ///   `buildkit` feature is off). BuildKit is what makes `RUN --mount=type=cache,target=…` work,
    ///   and mounting toolchain caches (cargo registry, `~/.nuget`, pnpm store, uv) is the difference
    ///   between a rebuild of seconds and one of minutes.
    /// - **`.dockerignore`.** Honored client-side by the CLI. Driving the HTTP endpoint means
    ///   assembling the context tar ourselves — and a naive tar of a real project root ships
    ///   `target/`/`node_modules/`/`bin/obj`, which is slow enough to be unusable.
    ///
    /// It costs nothing in requirements: the `docker` capability gate already probes `docker info`
    /// through this same CLI, so any test that can reach a daemon can run it (and
    /// `create_managed_network` sets the shell-out precedent).
    fn build_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_async_function(|_, opts: Table| {
            let spec = BuildSpec::from_table(&opts);
            async move { build(spec?).await }
        })
    }

    async fn build(spec: BuildSpec) -> mlua::Result<String> {
        let mut cmd = tokio::process::Command::new("docker");
        // The dockerfile is context-relative (Docker's rule); the CLI wants a path it can open, so
        // join it back onto the context for -f.
        cmd.arg("build")
            .arg("-f")
            .arg(std::path::Path::new(&spec.context).join(&spec.dockerfile))
            .arg("-t")
            .arg(&spec.tag);
        for (k, v) in &spec.buildargs {
            cmd.arg("--build-arg").arg(format!("{k}={v}"));
        }
        if let Some(target) = &spec.target {
            cmd.arg("--target").arg(target);
        }
        if spec.pull {
            cmd.arg("--pull");
        }
        if spec.nocache {
            cmd.arg("--no-cache");
        }
        cmd.arg(&spec.context);

        let output = cmd.output().await.map_err(derr)?;
        if !output.status.success() {
            // Carry the builder's own log. BuildKit writes progress and errors to stderr, but a
            // failing `RUN` prints the command's own output to stdout, so the diagnosis is usually
            // split across both — include each. Never hand back a tag for an image that isn't there.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let log = [stdout.trim(), stderr.trim()]
                .iter()
                .filter(|p| !p.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let status = match output.status.code() {
                Some(c) => format!("exit {c}"),
                None => "signalled".to_string(),
            };
            return Err(derr(format!(
                "build of `{}` failed ({}):\n{}",
                spec.dockerfile,
                status,
                tail(&log, 4000)
            )));
        }
        Ok(spec.tag)
    }

    /// Keep the last `n` characters of a build log — the error is at the end, and a full BuildKit
    /// transcript is far too long to carry in an error message.
    fn tail(s: &str, n: usize) -> &str {
        if s.len() <= n {
            return s;
        }
        match s.char_indices().nth(s.chars().count().saturating_sub(n)) {
            Some((at, _)) => &s[at..],
            None => s,
        }
    }

    /// `docker.network(opts?)` — create a user-defined bridge network (embedded DNS, so containers
    /// on it resolve each other by name/alias). `opts.name` overrides the generated unique name.
    fn network_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_async_function(|lua, opts: Option<Table>| {
            let name = match &opts {
                Some(t) => t.get::<Option<String>>("name"),
                None => Ok(None),
            };
            async move {
                let name = name?.unwrap_or_else(unique_network_name);
                let client = connect().await?;
                client
                    .create_network(CreateNetworkOptions {
                        name: name.clone(),
                        driver: "bridge".to_string(),
                        check_duplicate: true,
                        ..Default::default()
                    })
                    .await
                    .map_err(derr)?;
                lua.create_userdata(Network {
                    client,
                    name,
                    removed: false,
                })
            }
        })
    }

    /// Mint a managed user-defined bridge network **synchronously** — shelling out to
    /// `docker network create` (the same CLI the `Drop` backstop uses) so a *synchronous* caller can
    /// create one where it cannot `await` the bollard client. This is the seam a `prova.topology`'s
    /// lazy `ctx.network` field getter uses: the field is read from Lua synchronously, but the network
    /// it returns is the identical `Network` handle `docker.network()` yields — `.name`, async
    /// `stop()`, and the `Drop` backstop — so `ctx:manage`/scope teardown reaps it exactly the same.
    pub(crate) fn create_managed_network(lua: &Lua) -> mlua::Result<AnyUserData> {
        let name = unique_network_name();
        let output = std::process::Command::new("docker")
            .args(["network", "create", "--driver", "bridge", &name])
            .output()
            .map_err(derr)?;
        if !output.status.success() {
            return Err(derr(format!(
                "network create failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        // The one client that cannot negotiate: this seam is synchronous, and negotiation needs an
        // await. Harmless here — this handle only ever removes the network again, which every API
        // version in range agrees on. Everything version-sensitive (container create/start/inspect)
        // goes through `connect()`.
        let client = Docker::connect_with_local_defaults().map_err(derr)?;
        lua.create_userdata(Network {
            client,
            name,
            removed: false,
        })
    }

    struct Wait {
        port: Option<u16>,
        log: Option<String>,
        timeout: Duration,
        every: Duration,
    }

    struct Spec {
        image: String,
        /// Each published port: the container port and an optional *fixed* host port (else random).
        ports: Vec<(u16, Option<u16>)>,
        env: Vec<(String, String)>,
        command: Vec<String>,
        wait: Option<Wait>,
        /// Name of a user-defined network to join at create time (from a `docker.network` handle or
        /// a raw name). The container stays dual-homed: published `ports` still map to host ports.
        network: Option<String>,
        /// The alias to answer to on `network` (siblings resolve it by DNS). Requires `network`.
        alias: Option<String>,
        /// `HostConfig.extra_hosts` — `"name:ip"` entries added to the container's `/etc/hosts`. The
        /// C2 case is `"host.docker.internal:host-gateway"`: on native Linux `host-gateway` resolves
        /// to the bridge address the host is reachable at, which is how a containerized SUT reaches a
        /// host-bound mock. Docker Desktop provides the name anyway, so setting it always is a no-op
        /// there and keeps one code path across platforms.
        extra_hosts: Vec<String>,
        /// How many start attempts to spoil, pretending the runtime exposed the ports and bound
        /// nothing to them. **Crate-internal test hook — never parsed from Lua.**
        ///
        /// The defect it simulates happens about once in 750 container starts: far too rare to
        /// exercise by waiting, and far too consequential to leave unproven, since the recovery path
        /// had never once executed in a test run. Injection makes it reachable on demand. It is set
        /// only by tests constructing a `Spec` directly, so no user-facing surface grows a test-only
        /// knob and no ordinary run can reach it.
        fault_empty_binding: usize,
    }

    impl Spec {
        fn from_table(opts: &Table) -> mlua::Result<Spec> {
            let image = opts.get::<Option<String>>("image")?.ok_or_else(|| {
                mlua::Error::RuntimeError("docker.run requires an `image`".into())
            })?;
            // `ports` entries are either an integer container port (→ random host port) or a table
            // `{ container = N, host = M }` (→ fixed host port M, needed by e.g. Kafka's advertised
            // listener). A bare `{ N, M }` array works too.
            let mut ports: Vec<(u16, Option<u16>)> = Vec::new();
            if let Some(list) = opts.get::<Option<Vec<mlua::Value>>>("ports")? {
                for entry in list {
                    match entry {
                        mlua::Value::Integer(i) => ports.push((i as u16, None)),
                        mlua::Value::Table(t) => {
                            let container = t
                                .get::<Option<u16>>("container")?
                                .or(t.get::<Option<u16>>(1)?)
                                .ok_or_else(|| {
                                    mlua::Error::RuntimeError(
                                        "docker.run port table needs a container port".into(),
                                    )
                                })?;
                            let host = t.get::<Option<u16>>("host")?.or(t.get::<Option<u16>>(2)?);
                            ports.push((container, host));
                        }
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "docker.run ports entries must be integers or { container, host } tables".into(),
                            ))
                        }
                    }
                }
            }
            // `command` overrides the image's default CMD. Accept a string (whitespace-split) or a
            // list of args — e.g. "bin/pulsar standalone" or { "bin/pulsar", "standalone" }.
            let command = match opts.get::<mlua::Value>("command")? {
                mlua::Value::String(s) => s
                    .to_str()?
                    .split_whitespace()
                    .map(|w| w.to_string())
                    .collect(),
                mlua::Value::Table(t) => {
                    t.sequence_values::<String>().collect::<mlua::Result<_>>()?
                }
                _ => Vec::new(),
            };
            let mut env = Vec::new();
            if let Some(table) = opts.get::<Option<Table>>("env")? {
                for pair in table.pairs::<String, String>() {
                    let (k, v) = pair?;
                    env.push((k, v));
                }
            }
            let wait = match opts.get::<Option<Table>>("wait")? {
                None => None,
                Some(w) => Some(Wait {
                    port: w.get::<Option<u16>>("port")?,
                    log: w.get::<Option<String>>("log")?,
                    timeout: w
                        .get::<Option<String>>("timeout")?
                        .and_then(|s| parse_duration(&s))
                        .unwrap_or(Duration::from_secs(30)),
                    every: w
                        .get::<Option<String>>("every")?
                        .and_then(|s| parse_duration(&s))
                        .unwrap_or(Duration::from_millis(250)),
                }),
            };
            // `network` accepts a `docker.network` handle (read its `.name`) or a raw name string.
            const NETWORK_EXPECT: &str =
                "docker.run `network` must be a docker.network handle or a name string";
            let network = match opts.get::<Value>("network")? {
                Value::Nil => None,
                Value::String(s) => Some(s.to_str()?.to_string()),
                Value::UserData(ud) => {
                    let net = ud
                        .borrow::<Network>()
                        .map_err(|_| mlua::Error::RuntimeError(NETWORK_EXPECT.into()))?;
                    Some(net.name.clone())
                }
                other => {
                    let msg = format!("{NETWORK_EXPECT}, got {}", other.type_name());
                    return Err(mlua::Error::RuntimeError(msg));
                }
            };
            let alias = opts.get::<Option<String>>("alias")?;
            if alias.is_some() && network.is_none() {
                return Err(mlua::Error::RuntimeError(
                    "docker.run `alias` requires a `network`".into(),
                ));
            }
            let extra_hosts = opts
                .get::<Option<Vec<String>>>("extra_hosts")?
                .unwrap_or_default();
            Ok(Spec {
                image,
                ports,
                env,
                command,
                wait,
                network,
                alias,
                extra_hosts,
                // Never read from Lua: the fault hook is reachable only by a test building a `Spec`.
                fault_empty_binding: 0,
            })
        }
    }

    fn derr<E: std::fmt::Display>(e: E) -> mlua::Error {
        mlua::Error::RuntimeError(format!("docker: {e}"))
    }

    /// Process-wide counts of runtime misbehaviour prova papered over, exposed to Lua as
    /// `docker.diagnostics()`.
    ///
    /// These exist because recovery is **silent**, and a silent recovery is indistinguishable from
    /// nothing having gone wrong. For a soak measuring one container runtime against another, that
    /// distinction is the entire measurement: "2000 starts, all fine" and "2000 starts, 3 of which
    /// this runtime botched and we healed" are completely different findings about that runtime.
    ///
    /// They count the RUNTIME's failures, not prova's retries in general — nothing increments them
    /// on a healthy start.
    pub(crate) static PORT_BIND_RECOVERIES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static PORT_BIND_FAILURES: AtomicU64 = AtomicU64::new(0);

    /// Connect to the daemon, agreeing on an API version the way the `docker` CLI does.
    ///
    /// `connect_with_local_defaults` alone pins bollard's compiled-in default (v1.47 in 0.18) no
    /// matter what the daemon speaks — Docker Desktop 4.46 serves v1.51 — so prova was holding a
    /// different conversation with the daemon than the CLI was. Negotiating removes that variable:
    /// any behaviour difference between prova and `docker run` is then a difference in what we ask
    /// for, not in which dialect we asked.
    ///
    /// Negotiation costs one `/version` round-trip and degrades safely: if it fails, keep the
    /// default client rather than turning a working daemon into a hard error.
    async fn connect() -> mlua::Result<Docker> {
        let client = Docker::connect_with_local_defaults().map_err(derr)?;
        Ok(client.negotiate_version().await.unwrap_or_else(|_| {
            Docker::connect_with_local_defaults().expect("reconnect after failed negotiation")
        }))
    }

    async fn start(spec: Spec) -> mlua::Result<Container> {
        let client = connect().await?;

        // Pull the image only if it isn't already local — `docker run`'s own rule. A locally-BUILT
        // image (docker.build) exists in no registry, so an unconditional pull fails it with a
        // misleading "pull access denied / repository does not exist"; and for a pulled image, a
        // tag that's already present skips a pointless registry round-trip.
        if client.inspect_image(&spec.image).await.is_err() {
            // Pull the image (drain the progress stream to completion).
            let (from_image, tag) = split_image(&spec.image);
            let mut pull = client.create_image(
                Some(CreateImageOptions {
                    from_image,
                    tag,
                    ..Default::default()
                }),
                None,
                None,
            );
            while let Some(item) = pull.next().await {
                item.map_err(derr)?;
            }
        }

        // Publish each container port to a random host port (host_port "0").
        let mut exposed: HashMap<String, HashMap<(), ()>> = HashMap::new();
        let mut bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        for (container, host) in &spec.ports {
            let key = format!("{container}/tcp");
            exposed.insert(key.clone(), HashMap::new());
            bindings.insert(
                key,
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".to_string()),
                    host_port: Some(
                        host.map(|h| h.to_string())
                            .unwrap_or_else(|| "0".to_string()),
                    ),
                }]),
            );
        }

        // Join a user-defined network at create time (so embedded DNS resolves the container by its
        // alias from the first moment) — independent of, and simultaneous with, host port publishing.
        let networking_config = spec.network.as_ref().map(|net_name| {
            let mut endpoint = EndpointSettings::default();
            if let Some(alias) = &spec.alias {
                endpoint.aliases = Some(vec![alias.clone()]);
            }
            let mut endpoints_config = HashMap::new();
            endpoints_config.insert(net_name.clone(), endpoint);
            NetworkingConfig { endpoints_config }
        });

        let config = Config {
            image: Some(spec.image.clone()),
            env: Some(spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
            cmd: (!spec.command.is_empty()).then(|| spec.command.clone()),
            exposed_ports: (!exposed.is_empty()).then_some(exposed),
            host_config: Some(HostConfig {
                port_bindings: (!bindings.is_empty()).then_some(bindings),
                extra_hosts: (!spec.extra_hosts.is_empty()).then(|| spec.extra_hosts.clone()),
                ..Default::default()
            }),
            networking_config,
            ..Default::default()
        };

        let requested: Vec<u16> = spec.ports.iter().map(|(p, _)| *p).collect();

        // Start, and recover from a runtime that exposes a port but binds nothing to it.
        //
        // Observed on Docker Desktop under load (~1 start in 750): the container runs, the daemon
        // reports the port key, and its binding list stays EMPTY indefinitely — `5432/tcp=[]`. That
        // is not the publish race above; it is a stable, wrong answer, so no amount of polling
        // helps, and the container can never be reached from the host. The only recovery is a new
        // container.
        //
        // Retries are SPACED, because measurement showed an immediate one is useless: a back-to-back
        // recreate hit the same empty binding, and two different test binaries failed inside one
        // run. The daemon's port plumbing wedges for a window rather than fumbling one container, so
        // a retry has to outlast the window, not merely follow it. Few attempts, growing gaps: long
        // enough to ride out a transient wedge, short enough that a port which genuinely cannot be
        // published still fails promptly and says so.
        const BACKOFF: [Duration; 2] = [Duration::from_millis(500), Duration::from_secs(2)];
        let attempts = BACKOFF.len() + 1;
        let mut id = String::new();
        let mut ports = HashMap::new();
        for attempt in 1..=attempts {
            let created = client
                .create_container(None::<CreateContainerOptions<String>>, config.clone())
                .await
                .map_err(derr)?;
            id = created.id;
            client
                .start_container(&id, None::<StartContainerOptions<String>>)
                .await
                .map_err(derr)?;

            // Short and best-effort: mappings are almost always already there, and anything merely
            // late is re-resolved on demand by `host_port`. Nothing here blocks a container that
            // never needed a host port.
            let mut scan = published_ports(&client, &id, &requested, Duration::from_secs(2)).await;

            // A container that has EXITED has no port bindings — the daemon clears them when it
            // stops. That is a container which finished, not a runtime that failed to bind, and the
            // two are indistinguishable from the port map alone.
            //
            // Conflating them was a real bug, and an expensive one to believe: a short-lived
            // container (`sleep 2`, shorter than this scan) reliably produced "this runtime exposed
            // a port and bound nothing to it", and prova then recreated a container that had simply
            // done its job. Measured on one machine, 800 concurrent starts on the same runtime: 7
            // such "defects" with a 2s lifetime, 0 with a 30s one, nothing else changed. It also
            // sent the diagnosis in exactly the wrong direction — the counters attributed our
            // misreading to Docker Desktop, and a `docker` CLI arm running the identical protocol
            // saw zero.
            if !scan.bound_empty.is_empty() && exited_status(&client, &id).await.is_some() {
                scan.bound_empty.clear();
            }

            // Test hook: pretend this attempt hit the runtime defect (see `Spec::fault_empty_binding`).
            if attempt <= spec.fault_empty_binding {
                scan.bound_empty = requested.iter().copied().collect();
                scan.found.clear();
            }
            ports = scan.found;
            if scan.bound_empty.is_empty() {
                // Recovering on a later attempt means the runtime botched an earlier one. Record it:
                // the caller sees a working container and would otherwise never learn this happened.
                if attempt > 1 {
                    PORT_BIND_RECOVERIES.fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
            if attempt == attempts {
                PORT_BIND_FAILURES.fetch_add(1, Ordering::Relaxed);
                let mut stuck: Vec<String> =
                    scan.bound_empty.iter().map(|p| p.to_string()).collect();
                stuck.sort();
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.run: this runtime exposed port(s) {} but bound nothing to them, on \
                     {attempts} attempts over {:?} — the container cannot be reached from the host",
                    stuck.join(", "),
                    BACKOFF.iter().sum::<Duration>(),
                )));
            }
            // Discard the unusable container before trying again, so a retry cannot leak one.
            let _ = client
                .remove_container(
                    &id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
            tokio::time::sleep(BACKOFF[attempt - 1]).await;
        }

        let container = Container {
            client: client.clone(),
            id: id.clone(),
            ports,
            requested,
            alias: spec.alias.clone(),
            stopped: false,
        };

        if let Some(wait) = spec.wait {
            wait_ready(&container, &wait).await?;
        }
        Ok(container)
    }

    /// Is anything LISTENing on `port`, on an address reachable from OUTSIDE the container?
    ///
    /// `/proc/net/tcp{,6}` is the container's own kernel accounting — it reports what the process
    /// inside actually bound, which is the only honest answer to "is it ready". A server bound to
    /// LOOPBACK inside a container answers only itself, so it is NOT ready for a sibling or for the
    /// host; init phases that briefly bind localhost before the real start are exactly the case a
    /// naive check waves through.
    ///
    /// Addresses are native-endian hex, so IPv4 127.0.0.1 (0x7F000001) renders as `0100007F` — the
    /// trailing octet pair is the address's FIRST octet. State `0A` is TCP_LISTEN.
    fn listening_on(proc_net: &str, port: u16) -> bool {
        proc_net.lines().any(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 4 || f[3] != "0A" {
                return false;
            }
            let Some((addr, p)) = f[1].rsplit_once(':') else {
                return false;
            };
            if u16::from_str_radix(p, 16).ok() != Some(port) {
                return false;
            }
            !is_loopback_hex(addr)
        })
    }

    fn is_loopback_hex(addr: &str) -> bool {
        match addr.len() {
            8 => addr.ends_with("7F"),                        // 127.0.0.0/8
            32 => addr == "00000000000000000000000001000000", // ::1
            _ => false,
        }
    }

    /// The mapped host port for `port` — the authoritative answer, for a caller that actually needs
    /// one. Cache hit is the overwhelmingly common path; a miss re-asks the daemon, because under
    /// load a mapping can arrive after `docker.run` returned.
    ///
    /// A port that was never requested fails immediately: waiting could not help, and this is a real
    /// case worth answering fast (a network-only resource legitimately publishes nothing, and
    /// `docker_readiness.lua` asserts exactly that via `pcall`).
    async fn resolved_host_port(container: &Container, port: u16) -> mlua::Result<u16> {
        if let Some(hp) = container.ports.get(&port) {
            return Ok(*hp);
        }
        if !container.requested.contains(&port) {
            return Err(mlua::Error::RuntimeError(format!(
                "container port {port} was not published (docker.run was not asked to publish it)"
            )));
        }
        let late = published_ports(
            &container.client,
            &container.id,
            &[port],
            Duration::from_secs(15),
        )
        .await;
        if let Some(hp) = late.found.get(&port) {
            return Ok(*hp);
        }
        // Say which of the three things went wrong, not just "not published": the daemon would not
        // answer, the container died, or the mapping genuinely never appeared.
        let why = match (
            late.last_error,
            exited_status(&container.client, &container.id).await,
        ) {
            (Some(err), _) => format!(" — docker did not answer: {err}"),
            (None, Some(status)) => format!(" — container {status}"),
            (None, None) => format!(
                " — container is running but the mapping never appeared (docker reported ports: {})",
                late.last_seen.as_deref().unwrap_or("<none>")
            ),
        };
        Err(mlua::Error::RuntimeError(format!(
            "container port {port} was not published{why}"
        )))
    }

    /// What the daemon's port map says about one requested port.
    ///
    /// The three cases must stay distinct, and telling them apart is the whole difficulty: they all
    /// read as "no host port" to a caller, but they call for opposite responses. `NotYet` means keep
    /// waiting; `BoundNothing` means waiting is futile and the container must be replaced.
    #[derive(Debug, PartialEq, Eq)]
    enum PortState {
        /// A host port is bound. The normal answer.
        Published(u16),
        /// The daemon has answered about this port, and its answer is that nothing is bound to it —
        /// either an explicit null or an empty binding list. A stable wrong answer, not a pending
        /// one: this is the runtime defect that no amount of polling fixes.
        BoundNothing,
        /// The port is not in the map at all — the mapping is still being wired. Poll again.
        NotYet,
    }

    /// Classify one wanted port against a daemon port map. Pure, so the distinction above can be
    /// proven against every shape the daemon produces without needing a daemon that misbehaves on
    /// cue — the misbehaviour being roughly a one-in-750 event.
    fn classify_port(ports: &HashMap<String, Option<Vec<PortBinding>>>, want: u16) -> PortState {
        match ports.get(&format!("{want}/tcp")) {
            Some(Some(binds)) => match binds
                .first()
                .and_then(|b| b.host_port.as_ref())
                .and_then(|s| s.parse::<u16>().ok())
            {
                Some(hp) => PortState::Published(hp),
                // Present, but bound to nothing: an empty list, or an entry whose host port is
                // missing or unparseable. The daemon has spoken and the answer is "nothing".
                None => PortState::BoundNothing,
            },
            Some(None) => PortState::BoundNothing,
            None => PortState::NotYet,
        }
    }

    /// Read the host ports the daemon has assigned so far, polling until every wanted port has a
    /// binding or `budget` runs out. Returns whatever it found — **never an error**.
    ///
    /// Publishing is **not atomic with `start`**: the container is running before the daemon has
    /// finished wiring its port mappings. Idle, that gap is imperceptible (measured: mappings are
    /// present on the first inspect); under load it stretches, and how far depends on the runtime.
    /// A single un-retried inspect therefore wins on one machine and loses on another — the
    /// "works on mine" failure this polls away.
    ///
    /// Returning partial results rather than failing is deliberate, and is the lesson from getting
    /// this wrong once: a missing mapping only matters to a caller that actually wants a host port.
    /// A network-only resource — reachable by alias, nothing published — is a legitimate topology
    /// member, and making publication an eager precondition failed those containers for a fact they
    /// never needed. Resolution is therefore best-effort here and authoritative in `host_port`.
    async fn published_ports(
        client: &Docker,
        id: &str,
        wanted: &[u16],
        budget: Duration,
    ) -> PortScan {
        const EVERY: Duration = Duration::from_millis(50);
        let deadline = Instant::now() + budget;
        let mut scan = PortScan::default();
        if wanted.is_empty() {
            return scan;
        }
        let mut exited = false;
        loop {
            match client.inspect_container(id, None).await {
                Ok(info) => {
                    scan.last_error = None;
                    // Liveness comes from the SAME response as the port map. It used to be a second
                    // `inspect` per iteration, which doubled this loop's load on the daemon for a
                    // fact the first call already carried — and this loop is not gentle: every 50ms,
                    // per container, across every worker.
                    exited = matches!(info.state.as_ref().and_then(|s| s.running), Some(false));
                    if let Some(ports) = info.network_settings.and_then(|ns| ns.ports) {
                        // Keep what the daemon actually said. When this whole scan comes up empty
                        // the raw map is the evidence that separates "the key is absent" (the
                        // mapping has not been wired yet) from `"9000/tcp": null` (the daemon
                        // accepted the container but the binding failed) — two different bugs that
                        // look identical from a missing-port error alone.
                        scan.last_seen = Some(
                            ports
                                .iter()
                                .map(|(k, v)| match v {
                                    Some(b) => format!("{k}={b:?}"),
                                    None => format!("{k}=null"),
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                        scan.bound_empty.clear();
                        for want in wanted {
                            if scan.found.contains_key(want) {
                                continue;
                            }
                            match classify_port(&ports, *want) {
                                PortState::Published(hp) => {
                                    scan.found.insert(*want, hp);
                                }
                                PortState::BoundNothing => {
                                    scan.bound_empty.insert(*want);
                                }
                                PortState::NotYet => {}
                            }
                        }
                    }
                }
                // Keep retrying — a daemon under load can refuse or time out a single inspect — but
                // REMEMBER why. Silently swallowing this is how "the port was never published" and
                // "we could not ask" become the same, undiagnosable message.
                Err(e) => scan.last_error = Some(e.to_string()),
            }
            // Stop early on success, on a dead container (it will never publish anything more), or
            // when the budget is spent. The caller decides whether a partial answer is a problem.
            if scan.found.len() == wanted.len() || Instant::now() >= deadline || exited {
                return scan;
            }
            tokio::time::sleep(EVERY).await;
        }
    }

    /// What a port scan learned: the mappings found, and the last inspect error if the daemon was
    /// not answering — which is a different failure from a port that is genuinely unpublished.
    #[derive(Default)]
    struct PortScan {
        found: HashMap<u16, u16>,
        /// Ports the daemon reported with an EMPTY binding list — exposed, but bound to nothing.
        /// A stable wrong answer rather than a pending one, so the caller recreates instead of
        /// waiting. Recomputed each inspect, so it only ever describes the latest answer.
        bound_empty: std::collections::HashSet<u16>,
        last_error: Option<String>,
        /// The raw port map from the last successful inspect — evidence for the rare case where a
        /// running container never gets a mapping.
        last_seen: Option<String>,
    }

    /// The outcome of asking a container's own kernel whether a port is listening.
    ///
    /// The three cases must stay distinct. Collapsing `Failed` into `Unsupported` (as a bare
    /// `Option` does) means one slow or refused exec — routine while a container is still coming
    /// up — permanently downgrades readiness to the coarse host-port check for the rest of the wait.
    enum Probe {
        /// The container answered: this is the truth about whether the port is listening.
        Answered(bool),
        /// The image has no `cat`/procfs (scratch, distroless). It can never answer; stop asking.
        Unsupported,
        /// The exec itself failed — container not accepting execs *yet*, or a transient daemon
        /// error. Says nothing about readiness, and nothing about future attempts. Ask again.
        Failed,
    }

    /// Ask the container's kernel whether `port` is listening.
    async fn listening_in_container(container: &Container, port: u16) -> Probe {
        let cmd = vec![
            "cat".to_string(),
            "/proc/net/tcp".to_string(),
            "/proc/net/tcp6".to_string(),
        ];
        // A missing /proc/net/tcp6 makes `cat` exit non-zero while still printing tcp — so judge by
        // the output, not the exit code.
        let Ok((_, out, _)) = container_exec(&container.client, &container.id, cmd, None).await
        else {
            return Probe::Failed;
        };
        if !out.contains("local_address") {
            return Probe::Unsupported; // not a procfs table: this image can never answer
        }
        Probe::Answered(listening_on(&out, port))
    }

    /// Is the container still running? `Some(status)` describes a container that has *stopped*, for
    /// use in an error; `None` means it is still running (or the daemon could not tell us, which we
    /// treat as "keep waiting" rather than inventing a failure).
    async fn exited_status(client: &Docker, id: &str) -> Option<String> {
        let state = client.inspect_container(id, None).await.ok()?.state?;
        // Treat "the daemon did not tell us whether it is running" as running — the same answer as
        // before, but reached deliberately. Writing this as `state.running?` silently produced it
        // via `?` on a `None`, so a response we failed to understand was indistinguishable from a
        // healthy container, and would have been reported as "running but the mapping never
        // appeared" — a confident, wrong diagnosis.
        match state.running {
            Some(false) => {}
            Some(true) | None => return None,
        }
        let code = state.exit_code.unwrap_or_default();
        Some(match state.error.filter(|e| !e.is_empty()) {
            Some(err) => format!("exited with code {code} ({err})"),
            None => format!("exited with code {code}"),
        })
    }

    async fn wait_ready(container: &Container, wait: &Wait) -> mlua::Result<()> {
        let deadline = Instant::now() + wait.timeout;
        // Whether the in-container probe is supported — latched OFF only on a definitive
        // `Unsupported`, never on a transient `Failed`.
        //
        // An image with no `cat`/procfs (scratch, distroless — `traefik/whoami` is one) can never
        // answer, and re-asking every 250ms fires hundreds of failing exec round-trips across a long
        // wait. That is not just waste: under parallel docker load it is slow enough to consume the
        // readiness budget itself, turning a cheap fallback into a timeout. So: ask once, and if the
        // image *cannot* answer, use the coarse host-port check for the rest of the wait. An exec
        // that merely failed is a different thing and must not latch anything.
        let mut probe_supported = true;
        loop {
            let ready = if let Some(port) = wait.port {
                // Ask the CONTAINER, not the host. Connecting to the mapped host port is worthless as
                // a readiness signal: Docker Desktop's port proxy binds and accepts the moment the
                // container starts, so the check passes while the server is still booting — and never
                // fails at all for a container that never listens. It also cannot see an UNPUBLISHED
                // port, which an in-network-only resource legitimately has.
                let asked = if probe_supported {
                    listening_in_container(container, port).await
                } else {
                    Probe::Unsupported
                };
                match asked {
                    Probe::Answered(listening) => listening,
                    // Retry next tick; this says nothing about readiness either way.
                    Probe::Failed => false,
                    // The image cannot answer (no `cat`/procfs). Fall back to the coarse host-port
                    // check — no worse than before, but do not pretend it is a true signal.
                    Probe::Unsupported => {
                        probe_supported = false;
                        match container.ports.get(&port) {
                            Some(&host_port) => {
                                tokio::net::TcpStream::connect(("127.0.0.1", host_port))
                                    .await
                                    .is_ok()
                            }
                            None => false,
                        }
                    }
                }
            } else if let Some(pattern) = &wait.log {
                container_logs(&container.client, &container.id)
                    .await?
                    .contains(pattern.as_str())
            } else {
                true
            };
            if ready {
                return Ok(());
            }
            // A container that has EXITED will never become ready. Waiting out the full timeout to
            // say "not ready" hides the actual failure (a bad command, a missing env var, a crash)
            // behind a slow, uninformative error. Check liveness only after the readiness probe came
            // back false, so a container that became ready and exited immediately still counts.
            if let Some(status) = exited_status(&container.client, &container.id).await {
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.run: container {} {status} before becoming ready{}",
                    container.id,
                    tail_logs(&container.client, &container.id).await
                )));
            }
            if Instant::now() >= deadline {
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.run: container {} not ready within {:?}{}",
                    container.id,
                    wait.timeout,
                    tail_logs(&container.client, &container.id).await
                )));
            }
            tokio::time::sleep(wait.every).await;
        }
    }

    /// The last few log lines, formatted for appending to a readiness error — the single most useful
    /// thing to know when a container did not come up. Best-effort: a container whose logs cannot be
    /// read still produces the underlying error, just without this context.
    async fn tail_logs(client: &Docker, id: &str) -> String {
        const KEEP: usize = 10;
        let Ok(logs) = container_logs(client, id).await else {
            return String::new();
        };
        let lines: Vec<&str> = logs.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return "\n  (container produced no log output)".to_string();
        }
        let start = lines.len().saturating_sub(KEEP);
        let shown = lines[start..]
            .iter()
            .map(|l| format!("\n  | {l}"))
            .collect::<String>();
        format!("\n  last {} log line(s):{shown}", lines.len() - start)
    }

    async fn container_logs(client: &Docker, id: &str) -> mlua::Result<String> {
        let mut stream = client.logs(
            id,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                follow: false,
                tail: "all".to_string(),
                ..Default::default()
            }),
        );
        let mut out = String::new();
        while let Some(item) = stream.next().await {
            out.push_str(&log_text(item.map_err(derr)?));
        }
        Ok(out)
    }

    /// Exec `cmd` (an argv vector) in the container, optionally writing `stdin` to the process, and
    /// collect `(exit_code, stdout, stderr)`. `cmd` is run directly (no shell) — the caller passes
    /// `["sh", "-c", "<script>"]` when it genuinely wants a shell. `stdin` is written in full and the
    /// input closed (EOF) before output is drained, which suits non-interactive tools that read stdin
    /// to completion then emit (a producer, `mc pipe`, …); it is not meant for large streaming input.
    async fn container_exec(
        client: &Docker,
        id: &str,
        cmd: Vec<String>,
        stdin: Option<String>,
    ) -> mlua::Result<(i64, String, String)> {
        let want_stdin = stdin.is_some();
        let exec = client
            .create_exec(
                id,
                CreateExecOptions {
                    cmd: Some(cmd),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    attach_stdin: Some(want_stdin),
                    ..Default::default()
                },
            )
            .await
            .map_err(derr)?;
        let (mut stdout, mut stderr) = (String::new(), String::new());
        if let StartExecResults::Attached {
            mut output,
            mut input,
        } = client
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: false,
                    ..Default::default()
                }),
            )
            .await
            .map_err(derr)?
        {
            if let Some(data) = stdin {
                use tokio::io::AsyncWriteExt;
                input
                    .write_all(data.as_bytes())
                    .await
                    .map_err(|e| derr(bollard::errors::Error::IOError { err: e }))?;
                let _ = input.shutdown().await;
            }
            drop(input);
            while let Some(item) = output.next().await {
                match item.map_err(derr)? {
                    LogOutput::StdOut { message } => {
                        stdout.push_str(&String::from_utf8_lossy(&message))
                    }
                    LogOutput::StdErr { message } => {
                        stderr.push_str(&String::from_utf8_lossy(&message))
                    }
                    _ => {}
                }
            }
        }
        let inspect = client.inspect_exec(&exec.id).await.map_err(derr)?;
        Ok((inspect.exit_code.unwrap_or(-1), stdout, stderr))
    }

    /// Parse `container:run` arguments off the Lua boundary into owned values (so nothing `!Send`
    /// crosses the `await`). A **string** command runs under `sh -c` (a shell — for pipes/globs); an
    /// **argv table** runs directly with no shell, so no quoting is needed. `opts.stdin` is piped in.
    fn parse_run_cmd(
        cmd: Value,
        opts: Option<Table>,
    ) -> mlua::Result<(Vec<String>, Option<String>)> {
        let argv = match cmd {
            Value::String(s) => vec!["sh".to_string(), "-c".to_string(), s.to_str()?.to_string()],
            Value::Table(t) => {
                let mut v = Vec::new();
                for item in t.sequence_values::<String>() {
                    v.push(item?);
                }
                if v.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        "container:run: empty argv table".into(),
                    ));
                }
                v
            }
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "container:run expects a string or an argv table, got {}",
                    other.type_name()
                )))
            }
        };
        let stdin = match opts {
            Some(o) => o.get::<Option<String>>("stdin")?,
            None => None,
        };
        Ok((argv, stdin))
    }

    fn log_text(log: LogOutput) -> String {
        let bytes = match log {
            LogOutput::StdOut { message }
            | LogOutput::StdErr { message }
            | LogOutput::StdIn { message }
            | LogOutput::Console { message } => message,
        };
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Split "postgres:16-alpine" -> ("postgres", "16-alpine"); default tag "latest". A ':' that is
    /// part of a registry host:port (has a '/' after it) is not a tag separator.
    fn split_image(image: &str) -> (String, String) {
        match image.rsplit_once(':') {
            Some((name, tag)) if !tag.contains('/') => (name.to_string(), tag.to_string()),
            _ => (image.to_string(), "latest".to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn bind(host_port: &str) -> PortBinding {
            PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some(host_port.to_string()),
            }
        }

        fn map(
            entries: Vec<(&str, Option<Vec<PortBinding>>)>,
        ) -> HashMap<String, Option<Vec<PortBinding>>> {
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect()
        }

        #[test]
        fn a_bound_port_reports_its_host_port() {
            let ports = map(vec![("5432/tcp", Some(vec![bind("55431")]))]);
            assert_eq!(classify_port(&ports, 5432), PortState::Published(55431));
        }

        /// The distinction the whole retry design rests on. An ABSENT key means the daemon has not
        /// wired the mapping yet (wait); a key present with nothing bound means the daemon has
        /// answered and the answer is "nothing" (replace the container). Collapsing these is what
        /// made a stable runtime defect look like a slow publish, and cost 15s of polling before a
        /// misleading timeout.
        #[test]
        fn an_absent_key_is_pending_but_an_empty_binding_is_a_verdict() {
            let pending = map(vec![]);
            assert_eq!(classify_port(&pending, 5432), PortState::NotYet);

            // `"5432/tcp": []` — observed on Docker Desktop under load, stable for 15s+.
            let empty_list = map(vec![("5432/tcp", Some(vec![]))]);
            assert_eq!(classify_port(&empty_list, 5432), PortState::BoundNothing);

            // `"5432/tcp": null` — the same verdict in the daemon's other spelling.
            let null_binding = map(vec![("5432/tcp", None)]);
            assert_eq!(classify_port(&null_binding, 5432), PortState::BoundNothing);
        }

        /// A binding that exists but carries no usable host port is still a verdict, not a wait:
        /// there is nothing to poll for.
        #[test]
        fn a_binding_without_a_usable_host_port_is_bound_nothing() {
            let no_port = map(vec![(
                "5432/tcp",
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".to_string()),
                    host_port: None,
                }]),
            )]);
            assert_eq!(classify_port(&no_port, 5432), PortState::BoundNothing);

            let junk = map(vec![("5432/tcp", Some(vec![bind("not-a-number")]))]);
            assert_eq!(classify_port(&junk, 5432), PortState::BoundNothing);
        }

        fn spec_with_fault(fault: usize) -> Spec {
            Spec {
                image: "alpine:3.20".to_string(),
                ports: vec![(80, None)],
                env: Vec::new(),
                command: vec!["sleep".to_string(), "20".to_string()],
                wait: None,
                network: None,
                alias: None,
                extra_hosts: Vec::new(),
                fault_empty_binding: fault,
            }
        }

        /// The recovery path, executed on purpose.
        ///
        /// It had never run in any test: the defect it handles appears about once in 750 container
        /// starts, so every green suite was green without touching it. This drives both outcomes —
        /// a spoiled attempt that recovers, and a permanently spoiled one that gives up — and
        /// checks that each is *counted*, because a silent recovery is invisible to the soak that
        /// needs to tell a healthy runtime from a sick one.
        ///
        /// Both cases live in one test on purpose: the counters are process-wide, so separate tests
        /// running in parallel in this binary would read each other's increments.
        #[test]
        fn injected_empty_bindings_recover_or_fail_loudly_and_are_counted() {
            if !crate::docker_runs_linux_containers() {
                eprintln!("skipping: docker is not available");
                return;
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            // One spoiled attempt: prova should replace the container and hand back a working one.
            let before = PORT_BIND_RECOVERIES.load(Ordering::Relaxed);
            let container = rt
                .block_on(start(spec_with_fault(1)))
                .expect("a single spoiled attempt must be recovered, not surfaced");
            assert!(
                container.ports.contains_key(&80),
                "recovered container should carry a real host port, got {:?}",
                container.ports
            );
            assert_eq!(
                PORT_BIND_RECOVERIES.load(Ordering::Relaxed) - before,
                1,
                "a recovery must be recorded — otherwise a soak cannot see the runtime misbehave"
            );
            drop(container);

            // Spoiled beyond the retry budget: give up, say why, and count it as a failure.
            let before_fail = PORT_BIND_FAILURES.load(Ordering::Relaxed);
            let before_recover = PORT_BIND_RECOVERIES.load(Ordering::Relaxed);
            let msg = match rt.block_on(start(spec_with_fault(99))) {
                Ok(_) => panic!("a permanently unbindable port must not be reported as success"),
                Err(e) => e.to_string(),
            };
            assert!(
                msg.contains("bound nothing"),
                "the error must name the runtime defect, got: {msg}"
            );
            assert_eq!(
                PORT_BIND_FAILURES.load(Ordering::Relaxed) - before_fail,
                1,
                "giving up must be counted"
            );
            assert_eq!(
                PORT_BIND_RECOVERIES.load(Ordering::Relaxed) - before_recover,
                0,
                "giving up is not a recovery"
            );
        }

        /// A container that finished is not a runtime that failed to bind.
        ///
        /// The daemon clears port bindings when a container stops, so a short-lived container looks
        /// exactly like the runtime defect: port requested, nothing bound. prova used to believe it,
        /// recreate a container that had simply done its job, and record the waste as evidence
        /// against the runtime. Measured: 800 concurrent starts on one runtime produced 7 such
        /// "defects" at a 2s container lifetime and 0 at 30s, nothing else changed — and a `docker`
        /// CLI arm running the identical protocol saw none, which is what proved the fault was ours.
        #[test]
        fn a_container_that_exited_is_not_counted_as_a_runtime_defect() {
            if !crate::docker_runs_linux_containers() {
                eprintln!("skipping: docker is not available");
                return;
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            let before_recover = PORT_BIND_RECOVERIES.load(Ordering::Relaxed);
            let before_fail = PORT_BIND_FAILURES.load(Ordering::Relaxed);

            // Exits immediately, so the port scan is overwhelmingly likely to meet a stopped
            // container with its bindings already cleared — the exact shape that used to be
            // misread.
            let mut spec = spec_with_fault(0);
            spec.command = vec!["true".to_string()];
            let result = rt.block_on(start(spec));

            // Whether the scan caught it before or after it exited is a race, and either outcome is
            // legitimate — what must NEVER happen is blaming the runtime for it.
            assert_eq!(
                PORT_BIND_RECOVERIES.load(Ordering::Relaxed) - before_recover,
                0,
                "a container that exited on its own must not be recorded as a runtime bind defect"
            );
            assert_eq!(
                PORT_BIND_FAILURES.load(Ordering::Relaxed) - before_fail,
                0,
                "a container that exited on its own must not be recorded as a bind failure"
            );
            drop(result);
        }

        /// Ports are matched exactly: another container port being published says nothing about
        /// the one asked for, and must not be mistaken for it.
        #[test]
        fn other_ports_do_not_answer_for_the_one_requested() {
            let ports = map(vec![
                ("80/tcp", Some(vec![bind("55000")])),
                ("5432/udp", Some(vec![bind("55001")])),
            ]);
            assert_eq!(classify_port(&ports, 5432), PortState::NotYet);
            assert_eq!(classify_port(&ports, 80), PortState::Published(55000));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// sql (postgres/mysql/sqlite namespaces over one generic Connection via sqlx's `Any` driver)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
mod sql {
    use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
    use sqlx::any::{AnyPoolOptions, AnyRow, AnyTypeInfoKind};
    use sqlx::{AnyPool, Column, Row};

    /// Which SQL engine a namespace fronts. Every engine's `client(url)` returns the same generic
    /// `Connection` (sqlx `Any` driver) — the namespace exists for discoverability and URL-scheme
    /// validation, not for a per-engine API.
    #[derive(Clone, Copy)]
    pub(crate) enum Engine {
        Sqlite,
    }

    impl Engine {
        fn name(self) -> &'static str {
            match self {
                Engine::Sqlite => "sqlite",
            }
        }
        fn schemes(self) -> &'static [&'static str] {
            match self {
                Engine::Sqlite => &["sqlite://", "sqlite:"],
            }
        }
    }

    /// A database connection pool from
    /// `sqlite.client(url)`. All three return this same type. Methods are async; pair with
    /// `ctx:manage(conn)` (or `ctx:defer(function() conn:close() end)`).
    struct Connection {
        pool: AnyPool,
    }

    impl UserData for Connection {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // Run a statement (INSERT/UPDATE/DDL); returns rows affected.
            methods.add_async_method(
                "execute",
                |_, this, (sql, params): (String, Option<Vec<Value>>)| {
                    let pool = this.pool.clone();
                    let binds = to_binds(params);
                    async move {
                        let binds = binds?;
                        let result = bound(&sql, &binds).execute(&pool).await.map_err(db_err)?;
                        Ok(result.rows_affected() as i64)
                    }
                },
            );

            // Run a query; returns a list of rows, each a table of column name -> value.
            methods.add_async_method(
                "query",
                |lua, this, (sql, params): (String, Option<Vec<Value>>)| {
                    let pool = this.pool.clone();
                    let binds = to_binds(params);
                    async move {
                        let binds = binds?;
                        let rows = bound(&sql, &binds).fetch_all(&pool).await.map_err(db_err)?;
                        let out = lua.create_table()?;
                        for (i, row) in rows.iter().enumerate() {
                            out.set(i + 1, row_to_table(&lua, row)?)?;
                        }
                        Ok(out)
                    }
                },
            );

            // Query returning a single scalar (first column of the first row), or nil.
            methods.add_async_method(
                "query_value",
                |lua, this, (sql, params): (String, Option<Vec<Value>>)| {
                    let pool = this.pool.clone();
                    let binds = to_binds(params);
                    async move {
                        let binds = binds?;
                        let row = bound(&sql, &binds)
                            .fetch_optional(&pool)
                            .await
                            .map_err(db_err)?;
                        match row {
                            Some(row) => match row.columns().first() {
                                Some(col) => {
                                    extract(&lua, &row, col.ordinal(), col.type_info().kind())
                                }
                                None => Ok(Value::Nil),
                            },
                            None => Ok(Value::Nil),
                        }
                    }
                },
            );

            methods.add_async_method("close", |_, this, ()| {
                let pool = this.pool.clone();
                async move {
                    pool.close().await;
                    Ok(())
                }
            });
        }
    }

    pub(crate) fn make(lua: &Lua, engine: Engine) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        table.set("client", client_fn(lua, engine)?)?;
        Ok(table)
    }

    fn client_fn(lua: &Lua, engine: Engine) -> mlua::Result<Function> {
        lua.create_async_function(move |lua, url: String| async move {
            let name = engine.name();
            if !engine.schemes().iter().any(|s| url.starts_with(s)) {
                return Err(mlua::Error::RuntimeError(format!(
                    "{name}.client: expected a {scheme} URL, got {url:?}",
                    scheme = engine.schemes()[0]
                )));
            }
            sqlx::any::install_default_drivers(); // idempotent
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .map_err(|e| mlua::Error::RuntimeError(format!("{name}.client {url:?}: {e}")))?;
            lua.create_userdata(Connection { pool })
        })
    }

    /// An owned bind parameter (converted off the Lua boundary so nothing borrows Lua across await).
    enum Bind {
        Int(i64),
        Float(f64),
        Bool(bool),
        Str(String),
        Null,
    }

    fn to_binds(params: Option<Vec<Value>>) -> mlua::Result<Vec<Bind>> {
        params
            .unwrap_or_default()
            .into_iter()
            .map(|v| match v {
                Value::Integer(i) => Ok(Bind::Int(i)),
                Value::Number(n) => Ok(Bind::Float(n)),
                Value::Boolean(b) => Ok(Bind::Bool(b)),
                Value::String(s) => Ok(Bind::Str(s.to_str()?.to_string())),
                Value::Nil => Ok(Bind::Null),
                other => Err(mlua::Error::RuntimeError(format!(
                    "sql: unsupported parameter type {}",
                    other.type_name()
                ))),
            })
            .collect()
    }

    fn bound<'q>(
        sql: &'q str,
        binds: &'q [Bind],
    ) -> sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>> {
        let mut q = sqlx::query(sql);
        for b in binds {
            q = match b {
                Bind::Int(i) => q.bind(*i),
                Bind::Float(f) => q.bind(*f),
                Bind::Bool(x) => q.bind(*x),
                Bind::Str(s) => q.bind(s.as_str()),
                Bind::Null => q.bind(Option::<String>::None),
            };
        }
        q
    }

    fn db_err(e: sqlx::Error) -> mlua::Error {
        mlua::Error::RuntimeError(format!("sql error: {e}"))
    }

    fn row_to_table(lua: &Lua, row: &AnyRow) -> mlua::Result<Table> {
        let table = lua.create_table()?;
        for col in row.columns() {
            let value = extract(lua, row, col.ordinal(), col.type_info().kind())?;
            table.set(col.name(), value)?;
        }
        Ok(table)
    }

    /// Extract one column as a Lua value, mapping SQL NULL to nil. Concrete SQL types use a precise
    /// decode; a column with no declared type (`Null` kind — e.g. SQLite `count(*)` and other
    /// computed columns) is probed by trying candidate types in order.
    fn extract(lua: &Lua, row: &AnyRow, i: usize, kind: AnyTypeInfoKind) -> mlua::Result<Value> {
        use AnyTypeInfoKind as K;
        let value = match kind {
            K::Null => return extract_untyped(lua, row, i),
            K::Bool => to_value(
                row.try_get::<Option<bool>, _>(i)
                    .map_err(db_err)?
                    .map(Value::Boolean),
            ),
            K::SmallInt => to_value(
                row.try_get::<Option<i16>, _>(i)
                    .map_err(db_err)?
                    .map(|n| Value::Integer(n as i64)),
            ),
            K::Integer => to_value(
                row.try_get::<Option<i32>, _>(i)
                    .map_err(db_err)?
                    .map(|n| Value::Integer(n as i64)),
            ),
            K::BigInt => to_value(
                row.try_get::<Option<i64>, _>(i)
                    .map_err(db_err)?
                    .map(Value::Integer),
            ),
            K::Real => to_value(
                row.try_get::<Option<f32>, _>(i)
                    .map_err(db_err)?
                    .map(|n| Value::Number(n as f64)),
            ),
            K::Double => to_value(
                row.try_get::<Option<f64>, _>(i)
                    .map_err(db_err)?
                    .map(Value::Number),
            ),
            K::Text => match row.try_get::<Option<String>, _>(i).map_err(db_err)? {
                Some(s) => Value::String(lua.create_string(&s)?),
                None => Value::Nil,
            },
            K::Blob => match row.try_get::<Option<Vec<u8>>, _>(i).map_err(db_err)? {
                Some(b) => Value::String(lua.create_string(b)?),
                None => Value::Nil,
            },
        };
        Ok(value)
    }

    fn to_value(opt: Option<Value>) -> Value {
        opt.unwrap_or(Value::Nil)
    }

    /// Probe a column of unknown declared type by trying candidate decodes in order. `Ok(None)` from
    /// a decode means a real SQL NULL → nil; an `Err` means "wrong type, try the next". Integer
    /// before float before bool keeps SQLite's dynamic integers integral.
    fn extract_untyped(lua: &Lua, row: &AnyRow, i: usize) -> mlua::Result<Value> {
        if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
            return Ok(v.map(Value::Integer).unwrap_or(Value::Nil));
        }
        if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
            return Ok(v.map(Value::Number).unwrap_or(Value::Nil));
        }
        if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
            return Ok(v.map(Value::Boolean).unwrap_or(Value::Nil));
        }
        if let Ok(v) = row.try_get::<Option<String>, _>(i) {
            return match v {
                Some(s) => Ok(Value::String(lua.create_string(&s)?)),
                None => Ok(Value::Nil),
            };
        }
        if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(i) {
            return Ok(Value::String(lua.create_string(b)?));
        }
        Ok(Value::Nil)
    }
}

// ---------------------------------------------------------------------------------------------
// grpc (async; native — no `grpcurl` binary. Plaintext-only in v1, like http.)
// ---------------------------------------------------------------------------------------------

// A *dynamic* gRPC client: it learns the server's schema at runtime via gRPC Server Reflection
// (so tests need no `.proto` files and no codegen), builds request messages from Lua tables against
// the fetched descriptors, invokes with a generic tonic codec over `DynamicMessage`, and decodes the
// reply back to a Lua table. This keeps prova a single self-contained binary — the whole point of
// going native instead of shelling out to `grpcurl`. The server must have reflection enabled; if it
// doesn't, `grpc.client` fails with a clear message (a proto-file path mode can layer on later).
#[cfg(feature = "grpc")]
mod grpc {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use mlua::{Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};
    use prost::Message as _;
    use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, SerializeOptions};
    use prost_types::FileDescriptorProto;
    use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
    use tonic::codegen::http::uri::PathAndQuery;
    use tonic::transport::Channel;
    use tonic::{Request, Status};

    use crate::model::parse_duration;

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let grpc = lua.create_table()?;
        // grpc.client(addr, { timeout = "30s" }) → a Client (reflection is performed here, once).
        grpc.set(
            "client",
            lua.create_async_function(|lua, (addr, opts): (String, Option<Table>)| async move {
                let timeout = opt_duration(&opts, "timeout")?;
                let channel = connect_channel(&addr).await?;
                let pool = build_pool(&channel).await?;
                lua.create_userdata(Client {
                    channel,
                    pool,
                    timeout,
                })
            })?,
        )?;
        // grpc.wait_for(addr, { timeout = "30s", every = "500ms" }) — poll until the server answers a
        // reflection ListServices (boot-then-probe, mirroring http.wait_for).
        grpc.set(
            "wait_for",
            lua.create_async_function(|_, (addr, opts): (String, Option<Table>)| async move {
                let timeout = opt_duration(&opts, "timeout")?.unwrap_or(Duration::from_secs(30));
                let every = opt_duration(&opts, "every")?.unwrap_or(Duration::from_millis(500));
                let deadline = Instant::now() + timeout;
                loop {
                    if let Ok(channel) = connect_channel(&addr).await {
                        if list_services(&channel).await.is_ok() {
                            return Ok(());
                        }
                    }
                    if Instant::now() >= deadline {
                        return Err(err(format!(
                            "grpc.wait_for timed out after {timeout:?} waiting for {addr}"
                        )));
                    }
                    tokio::time::sleep(every).await;
                }
            })?,
        )?;
        // grpc.mock(ctx, { proto = … }) → the `mock` facet on the grpc namespace. Unlike `client`, it
        // must be told its schema: reflection teaches a client about a server, and a mock *is* the
        // server, so there is nobody to learn from.
        #[cfg(feature = "grpc-mock")]
        grpc.set("mock", super::grpc_mock::mock_fn(lua)?)?;
        Ok(grpc)
    }

    /// A connected client bound to one server. `client:call(method, req)` returns the response as a
    /// table; `client:call_status(method, req)` returns `{ ok, code, message, response }` so a test
    /// can assert on gRPC status codes (e.g. `NotFound`, `InvalidArgument`) without raising.
    struct Client {
        channel: Channel,
        pool: DescriptorPool,
        timeout: Option<Duration>,
    }

    impl UserData for Client {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_async_method(
                "call",
                |lua, this, (method, req): (String, Option<Value>)| async move {
                    let input = build_request(&lua, &this.pool, &method, req)?;
                    match invoke(&this.channel, &this.pool, &method, input, this.timeout).await {
                        Ok(msg) => response_to_lua(&lua, &msg),
                        Err(status) => Err(err(format!(
                            "grpc call {method} failed: {} ({})",
                            status.message(),
                            status.code()
                        ))),
                    }
                },
            );
            methods.add_async_method(
                "call_status",
                |lua, this, (method, req): (String, Option<Value>)| async move {
                    let input = build_request(&lua, &this.pool, &method, req)?;
                    let out = lua.create_table()?;
                    match invoke(&this.channel, &this.pool, &method, input, this.timeout).await {
                        Ok(msg) => {
                            out.set("ok", true)?;
                            out.set("code", "Ok")?;
                            out.set("message", "")?;
                            out.set("response", response_to_lua(&lua, &msg)?)?;
                        }
                        Err(status) => {
                            out.set("ok", false)?;
                            out.set("code", format!("{:?}", status.code()))?;
                            out.set("message", status.message().to_string())?;
                            out.set("response", Value::Nil)?;
                        }
                    }
                    Ok(out)
                },
            );
        }
    }

    async fn connect_channel(addr: &str) -> mlua::Result<Channel> {
        // Accept "host:port" or a full "http://host:port"; plaintext only in v1.
        let uri = if addr.contains("://") {
            addr.to_string()
        } else {
            format!("http://{addr}")
        };
        Channel::from_shared(uri)
            .map_err(|e| err(format!("grpc: invalid address {addr:?}: {e}")))?
            .connect()
            .await
            .map_err(|e| err(format!("grpc: could not connect to {addr}: {e}")))
    }

    /// Turn a Lua request table into a wire-ready `DynamicMessage` for `method`'s input type.
    fn build_request(
        lua: &Lua,
        pool: &DescriptorPool,
        method: &str,
        req: Option<Value>,
    ) -> mlua::Result<DynamicMessage> {
        let desc = method_descriptor(pool, method)?;
        let json: serde_json::Value = match req {
            Some(v) => lua.from_value(v)?,
            None => serde_json::Value::Object(Default::default()),
        };
        DynamicMessage::deserialize(desc.input(), &json)
            .map_err(|e| err(format!("grpc: building request for {method}: {e}")))
    }

    /// Serialize a response message to a Lua table. `skip_default_fields(false)` keeps zero/empty
    /// fields present so assertions can see the full message shape. Field names mirror how requests
    /// are written — proto (snake_case) names, not proto3-JSON camelCase — and 64-bit ints arrive
    /// as Lua numbers rather than strings (tests assert `res.id`, not `res.id == "3"`; Lua numbers
    /// are exact through 2^53, far beyond any test-scale id).
    fn response_to_lua(lua: &Lua, msg: &DynamicMessage) -> mlua::Result<Value> {
        let opts = SerializeOptions::new()
            .skip_default_fields(false)
            .use_proto_field_name(true)
            .stringify_64_bit_integers(false);
        let value = msg
            .serialize_with_options(serde_json::value::Serializer, &opts)
            .map_err(|e| err(format!("grpc: decoding response: {e}")))?;
        lua.to_value(&value)
    }

    fn method_descriptor(
        pool: &DescriptorPool,
        method: &str,
    ) -> mlua::Result<prost_reflect::MethodDescriptor> {
        // Accept "pkg.Service/Method" or "/pkg.Service/Method".
        let trimmed = method.trim_start_matches('/');
        let (service, method_name) = trimmed.rsplit_once('/').ok_or_else(|| {
            err(format!(
                "grpc: method must be \"package.Service/Method\", got {method:?}"
            ))
        })?;
        let svc = pool.get_service_by_name(service).ok_or_else(|| {
            err(format!(
                "grpc: service {service:?} not found via reflection (known: {})",
                pool.services()
                    .map(|s| s.full_name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let method = svc.methods().find(|m| m.name() == method_name);
        method.ok_or_else(|| {
            err(format!(
                "grpc: method {method_name:?} not found on {service}"
            ))
        })
    }

    async fn invoke(
        channel: &Channel,
        pool: &DescriptorPool,
        method: &str,
        input: DynamicMessage,
        timeout: Option<Duration>,
    ) -> Result<DynamicMessage, Status> {
        let desc = method_descriptor(pool, method).map_err(|e| Status::internal(e.to_string()))?;
        let path: PathAndQuery = format!("/{}/{}", desc.parent_service().full_name(), desc.name())
            .parse()
            .map_err(|e| Status::internal(format!("grpc: bad method path: {e}")))?;
        let mut grpc = tonic::client::Grpc::new(channel.clone());
        grpc.ready()
            .await
            .map_err(|e| Status::unavailable(format!("grpc: service not ready: {e}")))?;
        let codec = DynCodec {
            decode_into: desc.output(), // a client decodes the reply
        };
        let mut request = Request::new(input);
        if let Some(t) = timeout {
            request.set_timeout(t);
        }
        let resp = grpc.unary(request, path, codec).await?;
        Ok(resp.into_inner())
    }

    // A tonic codec that speaks `DynamicMessage` on both ends: the encoder just prost-encodes
    // whatever message it is handed; the decoder builds an empty message of a known descriptor and
    // merges the incoming bytes into it. This is the whole trick that lets one client call any
    // method dynamically.
    //
    // It is direction-agnostic, which is why `grpc.mock` shares it rather than owning a mirror copy:
    // the only thing that differs between the two ends is *what to decode into* — a client decodes
    // the method's output (the reply), a server decodes its input (the request). Hence
    // `decode_into` rather than `output`: naming it for the reply was naming it for one caller.
    #[derive(Clone)]
    pub(super) struct DynCodec {
        pub(super) decode_into: MessageDescriptor,
    }

    impl Codec for DynCodec {
        type Encode = DynamicMessage;
        type Decode = DynamicMessage;
        type Encoder = DynEncoder;
        type Decoder = DynDecoder;
        fn encoder(&mut self) -> DynEncoder {
            DynEncoder
        }
        fn decoder(&mut self) -> DynDecoder {
            DynDecoder {
                decode_into: self.decode_into.clone(),
            }
        }
    }

    pub(super) struct DynEncoder;
    impl Encoder for DynEncoder {
        type Item = DynamicMessage;
        type Error = Status;
        fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
            item.encode(dst)
                .map_err(|e| Status::internal(format!("grpc: encoding message: {e}")))
        }
    }

    pub(super) struct DynDecoder {
        decode_into: MessageDescriptor,
    }
    impl Decoder for DynDecoder {
        type Item = DynamicMessage;
        type Error = Status;
        fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
            let mut msg = DynamicMessage::new(self.decode_into.clone());
            msg.merge(src)
                .map_err(|e| Status::internal(format!("grpc: decoding message: {e}")))?;
            Ok(Some(msg))
        }
    }

    // -- reflection ---------------------------------------------------------------------------

    #[derive(Clone, Copy)]
    enum Rv {
        V1,
        V1alpha,
    }

    /// Build a descriptor pool for every service the server advertises, via reflection. Negotiates
    /// the reflection protocol version (v1, falling back to the older v1alpha many servers still use).
    async fn build_pool(channel: &Channel) -> mlua::Result<DescriptorPool> {
        let (services, rv) = list_services_negotiated(channel).await?;
        let mut files: HashMap<String, FileDescriptorProto> = HashMap::new();
        for service in &services {
            // The reflection service describes itself; skip it — we only want the app's schema.
            if service.starts_with("grpc.reflection.") {
                continue;
            }
            let raw = files_for_symbol(channel, rv, service).await.map_err(|e| {
                err(format!(
                    "grpc: reflecting {service}: {} ({})",
                    e.message(),
                    e.code()
                ))
            })?;
            for bytes in raw {
                let fdp = FileDescriptorProto::decode(bytes.as_slice())
                    .map_err(|e| err(format!("grpc: decoding file descriptor: {e}")))?;
                let name = fdp.name().to_string();
                files.entry(name).or_insert(fdp);
            }
        }
        let mut pool = DescriptorPool::new();
        pool.add_file_descriptor_protos(files.into_values())
            .map_err(|e| err(format!("grpc: building descriptor pool: {e}")))?;
        Ok(pool)
    }

    /// Try to list services over v1; if the server hasn't implemented v1 reflection, retry v1alpha.
    async fn list_services_negotiated(channel: &Channel) -> mlua::Result<(Vec<String>, Rv)> {
        match list_services_v1(channel).await {
            Ok(s) => Ok((s, Rv::V1)),
            Err(status) if status.code() == tonic::Code::Unimplemented => {
                let s = list_services_v1alpha(channel).await.map_err(|e| {
                    err(format!(
                        "grpc: server reflection (v1alpha) failed: {} ({})",
                        e.message(),
                        e.code()
                    ))
                })?;
                Ok((s, Rv::V1alpha))
            }
            Err(status) => Err(err(format!(
                "grpc: server reflection failed ({}). The server must enable gRPC reflection for \
                 prova's dynamic client. {}",
                status.code(),
                status.message()
            ))),
        }
    }

    /// Version-agnostic `list_services` used by `wait_for` (v1, then v1alpha).
    async fn list_services(channel: &Channel) -> mlua::Result<Vec<String>> {
        list_services_negotiated(channel).await.map(|(s, _)| s)
    }

    async fn files_for_symbol(
        channel: &Channel,
        rv: Rv,
        symbol: &str,
    ) -> Result<Vec<Vec<u8>>, Status> {
        match rv {
            Rv::V1 => files_for_symbol_v1(channel, symbol).await,
            Rv::V1alpha => files_for_symbol_v1alpha(channel, symbol).await,
        }
    }

    // The two reflection protocol versions have structurally identical messages under different
    // module paths; this macro generates the list/file-fetch pair for each so the orchestration above
    // stays version-agnostic.
    macro_rules! reflection_ops {
        ($modpath:ident, $list_fn:ident, $files_fn:ident) => {
            async fn $list_fn(channel: &Channel) -> Result<Vec<String>, Status> {
                use tonic_reflection::pb::$modpath::{
                    server_reflection_client::ServerReflectionClient,
                    server_reflection_request::MessageRequest,
                    server_reflection_response::MessageResponse, ServerReflectionRequest,
                };
                let mut client = ServerReflectionClient::new(channel.clone());
                let req = ServerReflectionRequest {
                    host: String::new(),
                    message_request: Some(MessageRequest::ListServices(String::new())),
                };
                let stream = futures::stream::iter(std::iter::once(req));
                let mut inner = client.server_reflection_info(stream).await?.into_inner();
                let mut out = Vec::new();
                while let Some(resp) = inner.message().await? {
                    match resp.message_response {
                        Some(MessageResponse::ListServicesResponse(list)) => {
                            out.extend(list.service.into_iter().map(|s| s.name));
                        }
                        Some(MessageResponse::ErrorResponse(e)) => {
                            return Err(Status::new(
                                tonic::Code::from(e.error_code),
                                e.error_message,
                            ));
                        }
                        _ => {}
                    }
                }
                Ok(out)
            }

            async fn $files_fn(channel: &Channel, symbol: &str) -> Result<Vec<Vec<u8>>, Status> {
                use tonic_reflection::pb::$modpath::{
                    server_reflection_client::ServerReflectionClient,
                    server_reflection_request::MessageRequest,
                    server_reflection_response::MessageResponse, ServerReflectionRequest,
                };
                let mut client = ServerReflectionClient::new(channel.clone());
                let req = ServerReflectionRequest {
                    host: String::new(),
                    message_request: Some(MessageRequest::FileContainingSymbol(symbol.to_string())),
                };
                let stream = futures::stream::iter(std::iter::once(req));
                let mut inner = client.server_reflection_info(stream).await?.into_inner();
                let mut out = Vec::new();
                while let Some(resp) = inner.message().await? {
                    match resp.message_response {
                        Some(MessageResponse::FileDescriptorResponse(fdr)) => {
                            out.extend(fdr.file_descriptor_proto);
                        }
                        Some(MessageResponse::ErrorResponse(e)) => {
                            return Err(Status::new(
                                tonic::Code::from(e.error_code),
                                e.error_message,
                            ));
                        }
                        _ => {}
                    }
                }
                Ok(out)
            }
        };
    }

    reflection_ops!(v1, list_services_v1, files_for_symbol_v1);
    reflection_ops!(v1alpha, list_services_v1alpha, files_for_symbol_v1alpha);

    fn opt_duration(opts: &Option<Table>, key: &str) -> mlua::Result<Option<Duration>> {
        Ok(match opts {
            Some(t) => t
                .get::<Option<String>>(key)?
                .and_then(|s| parse_duration(&s)),
            None => None,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// grpc_mock — the `mock` facet on the grpc namespace (`grpc.mock`)
// ---------------------------------------------------------------------------------------------

/// `grpc.mock` — a real gRPC server, in this process, that you stub and then assert on.
///
/// **The client's central trick does not invert, and that is the whole design problem.**
/// `grpc.client` needs no `.proto` because it learns the schema *from the server* over reflection. A
/// mock **is** the server: there is nobody to learn from, so it must be told. `proto` compiles a
/// `.proto` at runtime via `protox` — pure Rust, no `protoc` on PATH, which keeps the module's
/// promise ("no codegen") intact on the server side too. (A `FileDescriptorSet` and harvesting from
/// a live service are the other two sources; see `docs/plans/mocks.md` §6.)
///
/// **The mock serves reflection itself**, from the real `tonic-reflection` server. That is what lets
/// the *unmodified* `grpc.client` drive it with no special case — and it is the honest bar: if the
/// real client cannot tell the mock from a server, it is a server.
///
/// **Lua handlers survive the trip to HTTP/2**, which was not obvious. Two properties make it work,
/// and both are load-bearing: `tonic::server::UnaryService::Future` carries **no `Send` bound** (only
/// the request body must be Send, and hyper's `Incoming` is), and hyper's http2 delegates spawning to
/// a generic `E: Executor` that is likewise unbounded — so a `LocalExec` built on `spawn_local` keeps
/// the whole connection on the Lua thread. Reflection, which never touches Lua, is free to keep its
/// `Send` boxed future right next to it.
#[cfg(feature = "grpc-mock")]
mod grpc_mock {
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::time::Duration;

    use mlua::{
        Function, Lua, LuaSerdeExt, ObjectLike, Table, UserData, UserDataFields, UserDataMethods,
        Value,
    };
    use prost_reflect::{
        DescriptorPool, DeserializeOptions, DynamicMessage, MethodDescriptor, SerializeOptions,
    };
    use tonic::codegen::Service as _;
    use tonic::{Code, Request as TonicRequest, Response as TonicResponse, Status};

    use super::grpc::DynCodec;
    use crate::model::parse_duration;

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    /// hyper's http2 spawns per-stream tasks through this. The stock `TokioExecutor` uses
    /// `tokio::spawn` and would force `Send` all the way down to the Lua handler; `spawn_local`
    /// keeps every stream on the thread that owns the Lua state.
    #[derive(Clone)]
    struct LocalExec;

    impl<F> hyper::rt::Executor<F> for LocalExec
    where
        F: Future<Output = ()> + 'static,
    {
        fn execute(&self, fut: F) {
            tokio::task::spawn_local(fut);
        }
    }

    /// A resolved answer. `response` stays JSON rather than a built `DynamicMessage` because a stub
    /// may match several methods (`method_matches`), so the output descriptor to build against is
    /// not known until a call actually arrives.
    #[derive(Clone)]
    struct ReplySpec {
        code: Code,
        message: String,
        response: Option<serde_json::Value>,
        delay: Option<Duration>,
    }

    #[derive(Clone)]
    enum Reply {
        Unset,
        Data(ReplySpec),
        Handler(Function),
    }

    struct Stub {
        method: Option<String>,
        method_matches: Option<String>,
        reply: Reply,
    }

    struct Recorded {
        method: String,
        request: serde_json::Value,
        code: String,
        matched: bool,
        error: Option<String>,
    }

    #[derive(Default)]
    struct MockState {
        stubs: Vec<Stub>,
        journal: Vec<Recorded>,
        /// See the http facet: errors from our own stubs, tracked apart from a status the mock
        /// legitimately answered with.
        handler_errors: Vec<String>,
        allow_handler_errors: bool,
    }

    type Shared = Rc<RefCell<MockState>>;

    struct GrpcMock {
        url: String,
        host: String,
        port: u16,
        /// See the http facet: the host-gateway name a container reaches this mock at when `network`
        /// was requested. `None` → loopback-only.
        network_host: Option<String>,
        state: Shared,
        shutdown: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    struct StubHandle {
        state: Shared,
        idx: usize,
    }

    /// The client's options, mirrored: what `call_status` *reports* is what `:reply` *takes*. One
    /// spelling in both directions, so a test reads the same as the failure it reproduces.
    fn serialize_opts() -> SerializeOptions {
        SerializeOptions::new()
            .skip_default_fields(false)
            .use_proto_field_name(true)
            .stringify_64_bit_integers(false)
    }

    fn deserialize_opts() -> DeserializeOptions {
        DeserializeOptions::new().deny_unknown_fields(false)
    }

    pub(crate) fn mock_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_function(|lua, (ctx, opts): (Value, Option<Table>)| {
            let opts = opts.ok_or_else(|| {
                err(
                    "grpc.mock(ctx, opts): a mock must be told its schema — pass `proto = \"…\"`. \
                     Unlike grpc.client, it cannot learn one by reflection: it *is* the server.",
                )
            })?;
            let server = start(lua, &opts)?;
            let ud = lua.create_userdata(server)?;
            match ctx {
                Value::UserData(c) => {
                    let _: Value = c.call_method("manage", &ud)?;
                }
                Value::Nil => return Err(err(
                    "grpc.mock(ctx, opts): pass the test or fixture context (`t` / `ctx`) so the \
                         server is torn down with the scope",
                )),
                other => {
                    return Err(err(format!(
                        "grpc.mock(ctx, opts): expected the test or fixture context, got a {}",
                        other.type_name()
                    )))
                }
            }
            Ok(ud)
        })
    }

    /// Compile the schema, stand up reflection, bind, and spawn the accept loop. Everything that can
    /// fail does so here, synchronously — a bad `.proto` is an error at the `grpc.mock(…)` call site
    /// with the compiler's own diagnostic, not a mystery `Unimplemented` at the first call.
    fn start(lua: &Lua, opts: &Table) -> mlua::Result<GrpcMock> {
        let (pool, fds_bytes) = compile_schema(opts)?;
        let allow_handler_errors = opts
            .get::<Option<bool>>("allow_handler_errors")?
            .unwrap_or(false);
        // `network` — opt into a host-gateway vantage, binding all interfaces. Same contract as
        // http.mock: true → host.docker.internal, a string overrides the host name.
        let network_host: Option<String> = match opts.get::<Option<Value>>("network")? {
            Some(Value::Boolean(true)) => Some("host.docker.internal".to_string()),
            Some(Value::String(name)) => Some(name.to_string_lossy().to_string()),
            Some(Value::Boolean(false)) | None | Some(Value::Nil) => None,
            Some(other) => {
                return Err(err(format!(
                    "grpc.mock: `network` must be true or a host name, got a {}",
                    other.type_name()
                )))
            }
        };

        let reflect_v1 = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(&fds_bytes)
            .build_v1()
            .map_err(|e| err(format!("grpc.mock: building reflection service: {e}")))?;
        let reflect_v1alpha = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(&fds_bytes)
            .build_v1alpha()
            .map_err(|e| {
                err(format!(
                    "grpc.mock: building v1alpha reflection service: {e}"
                ))
            })?;

        let bind_ip = if network_host.is_some() {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        let std_listener = std::net::TcpListener::bind((bind_ip, 0))
            .map_err(|e| err(format!("grpc.mock: bind: {e}")))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| err(format!("grpc.mock: set_nonblocking: {e}")))?;
        let port = std_listener
            .local_addr()
            .map_err(|e| err(format!("grpc.mock: local_addr: {e}")))?
            .port();
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| err(format!("grpc.mock: from_std: {e}")))?;

        let state: Shared = Rc::new(RefCell::new(MockState {
            allow_handler_errors,
            ..Default::default()
        }));
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        let accept_state = state.clone();
        let accept_lua = lua.clone();
        tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else { break };
                        let io = hyper_util::rt::TokioIo::new(stream);
                        let conn_state = accept_state.clone();
                        let conn_lua = accept_lua.clone();
                        let conn_pool = pool.clone();
                        let r1 = reflect_v1.clone();
                        let r1a = reflect_v1alpha.clone();
                        tokio::task::spawn_local(async move {
                            let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                let state = conn_state.clone();
                                let lua = conn_lua.clone();
                                let pool = conn_pool.clone();
                                let mut r1 = r1.clone();
                                let mut r1a = r1a.clone();
                                async move {
                                    let path = req.uri().path().to_string();
                                    // Reflection is served by the real crate; it never touches Lua,
                                    // so its Send future sits happily inside this !Send one.
                                    let resp = if path.starts_with("/grpc.reflection.v1.ServerReflection/") {
                                        r1.call(req).await.unwrap_or_else(|e| match e {})
                                    } else if path.starts_with("/grpc.reflection.v1alpha.ServerReflection/") {
                                        r1a.call(req).await.unwrap_or_else(|e| match e {})
                                    } else {
                                        dispatch(lua, state, pool, &path, req).await
                                    };
                                    Ok::<_, std::convert::Infallible>(resp)
                                }
                            });
                            // gRPC is HTTP/2 with prior knowledge (no TLS, no upgrade) — exactly what
                            // http2::Builder::serve_connection does. LocalExec is what keeps the
                            // per-stream tasks off `tokio::spawn` and thus off the Send requirement.
                            let _ = hyper::server::conn::http2::Builder::new(LocalExec)
                                .serve_connection(io, svc)
                                .await;
                        });
                    }
                }
            }
        });

        Ok(GrpcMock {
            url: format!("http://127.0.0.1:{port}"),
            host: "127.0.0.1".to_string(),
            port,
            network_host,
            state,
            shutdown: RefCell::new(Some(tx)),
        })
    }

    /// `proto` → a descriptor pool + the encoded set reflection serves. Includes default to each
    /// file's own directory, which is what makes the common single-file case need no `includes` at
    /// all; declare them explicitly the moment an import crosses a directory.
    fn compile_schema(opts: &Table) -> mlua::Result<(DescriptorPool, Vec<u8>)> {
        let protos: Vec<String> = match opts.get::<Option<Value>>("proto")? {
            Some(Value::String(s)) => vec![s.to_string_lossy().to_string()],
            Some(Value::Table(t)) => {
                let mut v = Vec::new();
                for p in t.sequence_values::<String>() {
                    v.push(p?);
                }
                v
            }
            Some(other) => {
                return Err(err(format!(
                    "grpc.mock: `proto` must be a path or a list of paths, got a {}",
                    other.type_name()
                )))
            }
            None => return Err(err(
                "grpc.mock: pass `proto = \"path/to/service.proto\"` — a mock must be told the \
                     schema it serves",
            )),
        };
        if protos.is_empty() {
            return Err(err("grpc.mock: `proto` is empty"));
        }

        let mut includes: Vec<String> = Vec::new();
        if let Some(t) = opts.get::<Option<Table>>("includes")? {
            for p in t.sequence_values::<String>() {
                includes.push(p?);
            }
        }
        if includes.is_empty() {
            for p in &protos {
                if let Some(parent) = std::path::Path::new(p).parent() {
                    let d = parent.to_string_lossy().to_string();
                    if !d.is_empty() && !includes.contains(&d) {
                        includes.push(d);
                    }
                }
            }
        }

        let fds = protox::compile(&protos, &includes).map_err(|e| {
            // protox's own diagnostic names the file, line, and column. Surface it verbatim rather
            // than flattening it into "bad proto".
            err(format!("grpc.mock: compiling {protos:?}: {e}"))
        })?;
        let bytes = prost::Message::encode_to_vec(&fds);
        // Decode from bytes rather than converting types: it keeps this independent of whether
        // protox and prost-reflect happen to agree on a prost-types version.
        let pool = DescriptorPool::decode(bytes.as_slice())
            .map_err(|e| err(format!("grpc.mock: building descriptor pool: {e}")))?;
        Ok((pool, bytes))
    }

    /// Route one non-reflection request to the dynamic unary handler.
    async fn dispatch(
        lua: Lua,
        state: Shared,
        pool: DescriptorPool,
        path: &str,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> hyper::Response<tonic::body::Body> {
        // "/pkg.Service/Method" → "pkg.Service/Method"
        let full = path.trim_start_matches('/').to_string();
        let Some(desc) = lookup_method(&pool, &full) else {
            // A method the *schema* doesn't define. Distinct from one it defines that nobody
            // stubbed: this is "your test is wrong", that is "add a stub".
            return status_only(Status::new(
                Code::Unimplemented,
                format!("grpc.mock: no method {full:?} in the schema it was given"),
            ));
        };
        let codec = DynCodec {
            decode_into: desc.input(), // a server decodes the request
        };
        let mut grpc = tonic::server::Grpc::new(codec);
        let svc = DynService {
            lua,
            state,
            method: full,
            output: desc.output(),
        };
        grpc.unary(svc, req).await
    }

    fn lookup_method(pool: &DescriptorPool, full: &str) -> Option<MethodDescriptor> {
        let (service, method) = full.split_once('/')?;
        pool.get_service_by_name(service)?
            .methods()
            .find(|m| m.name() == method)
    }

    fn status_only(status: Status) -> hyper::Response<tonic::body::Body> {
        status.into_http()
    }

    /// The bridge from tonic's server machinery to Lua. Its `Future` is deliberately a plain
    /// (non-`Send`) boxed future — the property that makes this whole facet possible.
    struct DynService {
        lua: Lua,
        state: Shared,
        method: String,
        output: prost_reflect::MessageDescriptor,
    }

    impl tonic::server::UnaryService<DynamicMessage> for DynService {
        type Response = DynamicMessage;
        type Future =
            Pin<Box<dyn Future<Output = Result<TonicResponse<DynamicMessage>, Status>> + 'static>>;

        fn call(&mut self, request: TonicRequest<DynamicMessage>) -> Self::Future {
            let lua = self.lua.clone();
            let state = self.state.clone();
            let method = self.method.clone();
            let output = self.output.clone();
            Box::pin(async move { answer(lua, state, method, output, request.into_inner()).await })
        }
    }

    async fn answer(
        lua: Lua,
        state: Shared,
        method: String,
        output: prost_reflect::MessageDescriptor,
        request: DynamicMessage,
    ) -> Result<TonicResponse<DynamicMessage>, Status> {
        let req_json = message_to_json(&request)
            .map_err(|e| Status::internal(format!("grpc.mock: decoding request: {e}")))?;

        let matched_idx = match find_match(&lua, &state, &method) {
            Ok(i) => i,
            Err(e) => {
                record(
                    &state,
                    &method,
                    &req_json,
                    "Internal",
                    false,
                    Some(e.to_string()),
                );
                return Err(Status::internal(format!("grpc.mock: matching failed: {e}")));
            }
        };
        // Clone the reply out before awaiting into Lua: a handler may re-enter this same RefCell.
        let reply = matched_idx.map(|i| state.borrow().stubs[i].reply.clone());

        let (spec, error) = match reply {
            None => (
                ReplySpec {
                    code: Code::Unimplemented,
                    message: format!("grpc.mock: no stub for {method:?}"),
                    response: None,
                    delay: None,
                },
                None,
            ),
            Some(Reply::Unset) => (
                ReplySpec {
                    code: Code::Internal,
                    message: format!("grpc.mock: stub for {method:?} has no :reply(…)"),
                    response: None,
                    delay: None,
                },
                Some(format!("stub for {method:?} has no :reply(…)")),
            ),
            Some(Reply::Data(d)) => (d, None),
            Some(Reply::Handler(f)) => run_handler(&lua, f, &method, &req_json).await,
        };

        if let Some(d) = spec.delay {
            tokio::time::sleep(d).await;
        }

        if spec.code != Code::Ok {
            record(
                &state,
                &method,
                &req_json,
                &format!("{:?}", spec.code),
                matched_idx.is_some(),
                error,
            );
            return Err(Status::new(spec.code, spec.message));
        }

        let json = spec
            .response
            .unwrap_or(serde_json::Value::Object(Default::default()));
        let msg = DynamicMessage::deserialize_with_options(output, &json, &deserialize_opts())
            .map_err(|e| {
                let m = format!("grpc.mock: building the reply for {method:?}: {e}");
                record(
                    &state,
                    &method,
                    &req_json,
                    "Internal",
                    true,
                    Some(m.clone()),
                );
                Status::internal(m)
            })?;
        record(
            &state,
            &method,
            &req_json,
            "Ok",
            matched_idx.is_some(),
            error,
        );
        Ok(TonicResponse::new(msg))
    }

    async fn run_handler(
        lua: &Lua,
        f: Function,
        method: &str,
        req_json: &serde_json::Value,
    ) -> (ReplySpec, Option<String>) {
        let internal = |m: String| {
            (
                ReplySpec {
                    code: Code::Internal,
                    message: m.clone(),
                    response: None,
                    delay: None,
                },
                Some(m),
            )
        };
        let tbl = match req_to_lua(lua, method, req_json) {
            Ok(t) => t,
            Err(e) => return internal(format!("grpc.mock: handler input: {e}")),
        };
        match f.call_async::<Value>(tbl).await {
            Ok(Value::Table(t)) => match parse_reply(lua, &t) {
                Ok(s) => (s, None),
                Err(e) => internal(format!("grpc.mock: handler reply: {e}")),
            },
            Ok(other) => internal(format!(
                "grpc.mock: handler must return a reply table, returned a {}",
                other.type_name()
            )),
            Err(e) => internal(format!("grpc.mock: handler raised: {e}")),
        }
    }

    fn record(
        state: &Shared,
        method: &str,
        request: &serde_json::Value,
        code: &str,
        matched: bool,
        error: Option<String>,
    ) {
        // Only a *stub's* failure counts as a handler error. A mock that deliberately answers
        // `NotFound` is doing its job; a handler that raised is our bug.
        if let Some(e) = &error {
            state.borrow_mut().handler_errors.push(e.clone());
        }
        state.borrow_mut().journal.push(Recorded {
            method: method.to_string(),
            request: request.clone(),
            code: code.to_string(),
            matched,
            error,
        });
    }

    fn find_match(lua: &Lua, state: &Shared, method: &str) -> mlua::Result<Option<usize>> {
        let candidates: Vec<(usize, Option<String>)> = {
            let s = state.borrow();
            s.stubs
                .iter()
                .enumerate()
                .filter(|(_, stub)| stub.method.as_ref().is_none_or(|m| m == method))
                .map(|(i, stub)| (i, stub.method_matches.clone()))
                .collect()
        };
        for (i, pat) in candidates {
            match pat {
                None => return Ok(Some(i)),
                Some(p) => {
                    let string: Table = lua.globals().get("string")?;
                    let matcher: Function = string.get("match")?;
                    let r: Value = matcher.call((method.to_string(), p))?;
                    if !matches!(r, Value::Nil) {
                        return Ok(Some(i));
                    }
                }
            }
        }
        Ok(None)
    }

    fn message_to_json(msg: &DynamicMessage) -> Result<serde_json::Value, String> {
        let mut ser = serde_json::Serializer::new(Vec::new());
        msg.serialize_with_options(&mut ser, &serialize_opts())
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&ser.into_inner()).map_err(|e| e.to_string())
    }

    fn req_to_lua(lua: &Lua, method: &str, req_json: &serde_json::Value) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("method", method.to_string())?;
        t.set("request", lua.to_value(req_json)?)?;
        Ok(t)
    }

    fn recorded_to_lua(lua: &Lua, r: &Recorded) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("method", r.method.clone())?;
        t.set("request", lua.to_value(&r.request)?)?;
        t.set("code", r.code.clone())?;
        t.set("matched", r.matched)?;
        if let Some(e) = &r.error {
            t.set("error", e.clone())?;
        }
        Ok(t)
    }

    /// Parse `code` the way the client *prints* it (`format!("{:?}", status.code())` → `NotFound`),
    /// so what a failure reports is what you write to reproduce it. Accepted case-insensitively;
    /// an unknown name is rejected at the call site with the valid set, never silently downgraded to
    /// `Unknown` — a status that quietly became the wrong status is a test that lies.
    fn parse_code(name: &str) -> mlua::Result<Code> {
        const NAMES: &[(&str, Code)] = &[
            ("ok", Code::Ok),
            ("cancelled", Code::Cancelled),
            ("unknown", Code::Unknown),
            ("invalidargument", Code::InvalidArgument),
            ("deadlineexceeded", Code::DeadlineExceeded),
            ("notfound", Code::NotFound),
            ("alreadyexists", Code::AlreadyExists),
            ("permissiondenied", Code::PermissionDenied),
            ("resourceexhausted", Code::ResourceExhausted),
            ("failedprecondition", Code::FailedPrecondition),
            ("aborted", Code::Aborted),
            ("outofrange", Code::OutOfRange),
            ("unimplemented", Code::Unimplemented),
            ("internal", Code::Internal),
            ("unavailable", Code::Unavailable),
            ("dataloss", Code::DataLoss),
            ("unauthenticated", Code::Unauthenticated),
        ];
        let key = name.replace(['_', '-'], "").to_ascii_lowercase();
        NAMES
            .iter()
            .find(|(n, _)| *n == key)
            .map(|(_, c)| *c)
            .ok_or_else(|| {
                err(format!(
                    "grpc.mock: unknown status code {name:?}. Valid: Ok, Cancelled, Unknown, \
                     InvalidArgument, DeadlineExceeded, NotFound, AlreadyExists, PermissionDenied, \
                     ResourceExhausted, FailedPrecondition, Aborted, OutOfRange, Unimplemented, \
                     Internal, Unavailable, DataLoss, Unauthenticated"
                ))
            })
    }

    fn parse_reply(lua: &Lua, t: &Table) -> mlua::Result<ReplySpec> {
        let code = match t.get::<Option<String>>("code")? {
            Some(name) => parse_code(&name)?,
            None => Code::Ok,
        };
        let message = t.get::<Option<String>>("message")?.unwrap_or_default();
        let response = match t.get::<Option<Value>>("response")?.filter(|v| !v.is_nil()) {
            Some(v) => Some(lua.from_value::<serde_json::Value>(v)?),
            None => None,
        };
        if code != Code::Ok && response.is_some() {
            return Err(err(
                "grpc.mock reply: has both a non-Ok `code` and a `response` — an RPC answers with a \
                 message or a status, not both",
            ));
        }
        let delay = match t.get::<Option<String>>("delay")? {
            Some(s) => Some(
                parse_duration(&s)
                    .ok_or_else(|| err(format!("grpc.mock reply: bad `delay` duration {s:?}")))?,
            ),
            None => None,
        };
        Ok(ReplySpec {
            code,
            message,
            response,
            delay,
        })
    }

    impl UserData for GrpcMock {
        fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
            fields.add_field_method_get("url", |_, this| Ok(this.url.clone()));
            fields.add_field_method_get("host", |_, this| Ok(this.host.clone()));
            fields.add_field_method_get("port", |_, this| Ok(this.port));
            fields.add_field_method_get("network", |lua, this| {
                let Some(host) = &this.network_host else {
                    return Ok(Value::Nil);
                };
                let t = lua.create_table()?;
                t.set("url", format!("http://{host}:{}", this.port))?;
                t.set("host", host.clone())?;
                t.set("port", this.port)?;
                Ok(Value::Table(t))
            });
        }

        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("on", |lua, this, spec: Table| {
                let stub = Stub {
                    method: spec.get::<Option<String>>("method")?,
                    method_matches: spec.get::<Option<String>>("method_matches")?,
                    reply: Reply::Unset,
                };
                let idx = {
                    let mut s = this.state.borrow_mut();
                    s.stubs.push(stub);
                    s.stubs.len() - 1
                };
                lua.create_userdata(StubHandle {
                    state: this.state.clone(),
                    idx,
                })
            });

            methods.add_method("received", |lua, this, filter: Option<Table>| {
                let want_method = match &filter {
                    Some(f) => f.get::<Option<String>>("method")?,
                    None => None,
                };
                let out = lua.create_table()?;
                let s = this.state.borrow();
                let mut n = 0;
                for r in s.journal.iter() {
                    if let Some(m) = &want_method {
                        if &r.method != m {
                            continue;
                        }
                    }
                    n += 1;
                    out.set(n, recorded_to_lua(lua, r)?)?;
                }
                Ok(out)
            });

            // Raises on a reply-handler error, exactly as the http facet does — see there for why
            // this rides `ctx:manage` teardown rather than inventing a reporting path.
            methods.add_method("stop", |_, this, ()| {
                if let Some(tx) = this.shutdown.borrow_mut().take() {
                    let _ = tx.send(());
                }
                let errs = {
                    let mut s = this.state.borrow_mut();
                    if s.allow_handler_errors {
                        s.handler_errors.clear();
                        Vec::new()
                    } else {
                        std::mem::take(&mut s.handler_errors)
                    }
                };
                if !errs.is_empty() {
                    return Err(super::mock::handler_error_report("grpc.mock", &errs));
                }
                Ok(())
            });
        }
    }

    impl UserData for StubHandle {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("reply", |lua, this, v: Value| {
                let reply = match v {
                    Value::Function(f) => Reply::Handler(f),
                    Value::Table(t) => Reply::Data(parse_reply(lua, &t)?),
                    other => {
                        let msg = format!(
                            "grpc.mock :reply expects a reply table or a handler function, got a {}",
                            other.type_name()
                        );
                        return Err(err(msg));
                    }
                };
                this.state.borrow_mut().stubs[this.idx].reply = reply;
                Ok(())
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// yaml (sync — parse YAML text to Lua values; the counterpart to http's `:json()`)
// ---------------------------------------------------------------------------------------------

// A general capability for a cloud-oriented, polyglot world: k8s manifests, CI configs, and compose
// files are all YAML. `yaml.parse` handles a single document; `yaml.parse_all` handles a
// multi-document stream (`---`-separated), which is exactly what Kubernetes manifests use.
#[cfg(feature = "yaml")]
mod yaml {
    use mlua::{Lua, LuaSerdeExt, Table};
    use serde::Deserialize;

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let yaml = lua.create_table()?;

        // yaml.parse(text) → Lua value for the single/first document. Raises on invalid YAML.
        yaml.set(
            "parse",
            lua.create_function(|lua, text: String| {
                let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)
                    .map_err(|e| mlua::Error::RuntimeError(format!("yaml.parse: {e}")))?;
                lua.to_value(&value)
            })?,
        )?;

        // yaml.parse_all(text) → list of Lua values, one per `---`-separated document. Raises on the
        // first invalid document (with its 1-based index). An empty/whitespace-only string yields {}.
        yaml.set(
            "parse_all",
            lua.create_function(|lua, text: String| {
                let out = lua.create_table()?;
                for (i, doc) in serde_yaml_ng::Deserializer::from_str(&text).enumerate() {
                    let value = serde_yaml_ng::Value::deserialize(doc).map_err(|e| {
                        mlua::Error::RuntimeError(format!(
                            "yaml.parse_all: document {}: {e}",
                            i + 1
                        ))
                    })?;
                    out.push(lua.to_value(&value)?)?;
                }
                Ok(out)
            })?,
        )?;

        Ok(yaml)
    }
}

// ---------------------------------------------------------------------------------------------
// graphql (async; POST { query, variables } → { data, errors } over HTTP — the third transport)
// ---------------------------------------------------------------------------------------------

// GraphQL is one endpoint spoken over HTTP POST, so this is a thin, consistent layer: a client bound
// to a URL + headers, with `:query` (the happy path — returns `data`, raises if the response carries
// `errors`) and `:execute` (the full `{ data, errors, status }` envelope, for asserting on errors) —
// mirroring the grpc module's `call` / `call_status`. Queries and mutations share the transport.
#[cfg(feature = "graphql")]
mod graphql {
    use std::time::Duration;

    use mlua::{Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};

    use crate::model::parse_duration;

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    /// A GraphQL client bound to one endpoint. `client:query(q, vars?)` returns `data` (raising on
    /// `errors`); `client:execute(q, vars?)` returns `{ data, errors, status }` without raising.
    struct GraphqlClient {
        url: String,
        headers: Vec<(String, String)>,
        timeout: Option<Duration>,
    }

    /// An owned request spec (Lua-free) so nothing borrows Lua across the await.
    struct Request {
        url: String,
        headers: Vec<(String, String)>,
        timeout: Option<Duration>,
        body: Vec<u8>,
    }

    fn build_request(
        lua: &Lua,
        client: &GraphqlClient,
        query: String,
        variables: Option<Value>,
    ) -> mlua::Result<Request> {
        let mut payload = serde_json::Map::new();
        payload.insert("query".into(), serde_json::Value::String(query));
        if let Some(v) = variables {
            let vars: serde_json::Value = lua.from_value(v)?;
            if !vars.is_null() {
                payload.insert("variables".into(), vars);
            }
        }
        let body = serde_json::to_vec(&serde_json::Value::Object(payload))
            .map_err(|e| err(format!("graphql: encoding request: {e}")))?;
        Ok(Request {
            url: client.url.clone(),
            headers: client.headers.clone(),
            timeout: client.timeout,
            body,
        })
    }

    async fn send(req: Request) -> mlua::Result<(u16, serde_json::Value)> {
        let http = reqwest::Client::new();
        let mut r = http
            .post(&req.url)
            .header("content-type", "application/json")
            .body(req.body);
        for (k, v) in req.headers {
            r = r.header(k, v);
        }
        if let Some(t) = req.timeout {
            r = r.timeout(t);
        }
        let resp = r
            .send()
            .await
            .map_err(|e| err(format!("graphql request failed: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| err(format!("reading graphql response: {e}")))?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| err(format!("graphql response is not JSON: {e}")))?;
        Ok((status, json))
    }

    /// Convert a JSON value to Lua, mapping every `null` — top-level or nested — to Lua `nil`
    /// (mlua otherwise uses a null-sentinel lightuserdata, which no test author expects to meet:
    /// `t:expect(data.thing):is_nil()` must hold for a JSON null). Trade-off: a null INSIDE an
    /// array becomes a nil hole that ends the Lua sequence there; JSON APIs under test rarely
    /// return interior array nulls, and nil ergonomics win for assertions.
    fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
        let opts = mlua::SerializeOptions::new()
            .serialize_none_to_null(false)
            .serialize_unit_to_null(false);
        lua.to_value_with(v, opts)
    }

    /// Non-empty `errors` in the response, formatted for an error message (or `None` if clean).
    fn errors_of(json: &serde_json::Value) -> Option<String> {
        match json.get("errors") {
            Some(serde_json::Value::Array(a)) if !a.is_empty() => Some(
                a.iter()
                    .map(|e| {
                        e.get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| e.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            _ => None,
        }
    }

    impl UserData for GraphqlClient {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // query(query, variables?) → data table; raises if the response carries GraphQL errors.
            methods.add_async_method(
                "query",
                |lua, this, (query, variables): (String, Option<Value>)| {
                    let req = build_request(&lua, &this, query, variables);
                    async move {
                        let (_status, json) = send(req?).await?;
                        if let Some(errors) = errors_of(&json) {
                            return Err(err(format!("graphql errors: {errors}")));
                        }
                        let data = json.get("data").cloned().unwrap_or(serde_json::Value::Null);
                        json_to_lua(&lua, &data)
                    }
                },
            );

            // execute(query, variables?) → { data, errors, status }; never raises on GraphQL errors.
            methods.add_async_method(
                "execute",
                |lua, this, (query, variables): (String, Option<Value>)| {
                    let req = build_request(&lua, &this, query, variables);
                    async move {
                        let (status, json) = send(req?).await?;
                        let out = lua.create_table()?;
                        out.set("status", status)?;
                        out.set(
                            "data",
                            json_to_lua(
                                &lua,
                                &json.get("data").cloned().unwrap_or(serde_json::Value::Null),
                            )?,
                        )?;
                        out.set(
                            "errors",
                            json_to_lua(
                                &lua,
                                &json
                                    .get("errors")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            )?,
                        )?;
                        Ok(out)
                    }
                },
            );
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let graphql = lua.create_table()?;
        // graphql.client{ url, headers?, timeout? } → a client for one GraphQL endpoint.
        graphql.set(
            "client",
            lua.create_function(|lua, opts: Table| {
                let url = opts
                    .get::<Option<String>>("url")?
                    .ok_or_else(|| err("graphql.client requires a `url`"))?;
                let mut headers = Vec::new();
                if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
                    for pair in hdrs.pairs::<String, String>() {
                        let (k, v) = pair?;
                        headers.push((k, v));
                    }
                }
                let timeout = opts
                    .get::<Option<String>>("timeout")?
                    .and_then(|s| parse_duration(&s));
                lua.create_userdata(GraphqlClient {
                    url,
                    headers,
                    timeout,
                })
            })?,
        )?;
        Ok(graphql)
    }
}
