//! First-party capability modules injected as globals alongside `prova`: `shell`, `fs`, `http`,
//! `docker`, and the SQL engines.
//!
//! These are what make prova useful beyond testing itself — bring a system into existence and poke
//! it. `shell.run`/`shell.spawn`, `http.*`, and `docker.*` are async (child processes / requests /
//! docker calls never block the worker); `fs` is synchronous (fast metadata/read ops). All take
//! context explicitly (no ambient cwd), preserving the isolation the design promises. `http` is
//! behind a default-on feature and is HTTP-only in v1 (an `https`/TLS feature can layer on later);
//! `docker` uses the typed **bollard** daemon client, so tests that need it declare
//! `requires = { "docker" }` to skip gracefully where the daemon is absent. `http`/`postgres`/`docker` are
//! each behind a default-on feature so builds can opt out of their dependency trees.

use std::path::Path;
use std::time::Instant;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods};

use crate::model::parse_duration;

/// Install the built-in module globals (`shell`, `fs`, `docker`, and — with the `http` feature —
/// `http`) into `lua`.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set("shell", make_shell(lua)?)?;
    lua.globals().set("fs", make_fs(lua)?)?;
    lua.globals().set("net", make_net(lua)?)?;
    #[cfg(feature = "docker")]
    lua.globals().set("docker", docker::make(lua)?)?;
    #[cfg(feature = "http")]
    lua.globals().set("http", http::make(lua)?)?;
    #[cfg(feature = "postgres")]
    lua.globals().set("postgres", sql::make(lua, sql::Engine::Postgres)?)?;
    #[cfg(feature = "mysql")]
    lua.globals().set("mysql", sql::make(lua, sql::Engine::Mysql)?)?;
    #[cfg(feature = "sqlite")]
    lua.globals().set("sqlite", sql::make(lua, sql::Engine::Sqlite)?)?;
    #[cfg(feature = "grpc")]
    lua.globals().set("grpc", grpc::make(lua)?)?;
    #[cfg(feature = "graphql")]
    lua.globals().set("graphql", graphql::make(lua)?)?;
    #[cfg(feature = "yaml")]
    lua.globals().set("yaml", yaml::make(lua)?)?;
    #[cfg(feature = "redis")]
    lua.globals().set("redis", redis_mod::make(lua)?)?;
    #[cfg(feature = "pulsar")]
    lua.globals().set("pulsar", pulsar_mod::make(lua)?)?;
    #[cfg(feature = "kafka")]
    lua.globals().set("kafka", kafka_mod::make(lua)?)?;
    #[cfg(feature = "s3")]
    lua.globals().set("s3", s3_mod::make(lua)?)?;
    // Absent-namespace stubs: in a lean distribution a native namespace's feature may be off. Install
    // a stub so `kafka.client(...)` raises a clear "not compiled into this build" error instead of a
    // bare `attempt to index a nil value` — the call-side companion to the `requires` skip. In the
    // default build every feature is on, so none of these arms compile.
    #[cfg(not(feature = "docker"))]
    lua.globals().set("docker", absent_stub(lua, "docker")?)?;
    #[cfg(not(feature = "http"))]
    lua.globals().set("http", absent_stub(lua, "http")?)?;
    #[cfg(not(feature = "postgres"))]
    lua.globals().set("postgres", absent_stub(lua, "postgres")?)?;
    #[cfg(not(feature = "mysql"))]
    lua.globals().set("mysql", absent_stub(lua, "mysql")?)?;
    #[cfg(not(feature = "sqlite"))]
    lua.globals().set("sqlite", absent_stub(lua, "sqlite")?)?;
    #[cfg(not(feature = "grpc"))]
    lua.globals().set("grpc", absent_stub(lua, "grpc")?)?;
    #[cfg(not(feature = "graphql"))]
    lua.globals().set("graphql", absent_stub(lua, "graphql")?)?;
    #[cfg(not(feature = "yaml"))]
    lua.globals().set("yaml", absent_stub(lua, "yaml")?)?;
    #[cfg(not(feature = "redis"))]
    lua.globals().set("redis", absent_stub(lua, "redis")?)?;
    #[cfg(not(feature = "pulsar"))]
    lua.globals().set("pulsar", absent_stub(lua, "pulsar")?)?;
    #[cfg(not(feature = "kafka"))]
    lua.globals().set("kafka", absent_stub(lua, "kafka")?)?;
    #[cfg(not(feature = "s3"))]
    lua.globals().set("s3", absent_stub(lua, "s3")?)?;
    // The `prova.containerized` scaffolding helper — the ergonomic keystone every containerized
    // resource (first-party recipe or third-party plugin) is authored through. Always available;
    // the globals it composes (`docker`, `prova.retry`) resolve when a generated `container` is
    // *called*. Loaded before the recipes so they can be expressed in terms of it.
    lua.load(CONTAINERIZED_LUA)
        .set_name("@prova/containerized")
        .exec()?;
    // Resource recipes — Lua sugar over docker.run + prova.retry + postgres.client + ctx:manage. Loaded
    // after the modules exist; the globals they touch resolve when a recipe is *called*.
    #[cfg(feature = "postgres")]
    lua.load(POSTGRES_RECIPES_LUA)
        .set_name("@prova/postgres-recipes")
        .exec()?;
    #[cfg(feature = "mysql")]
    lua.load(MYSQL_RECIPES_LUA)
        .set_name("@prova/mysql-recipes")
        .exec()?;
    #[cfg(feature = "redis")]
    lua.load(REDIS_RECIPES_LUA)
        .set_name("@prova/redis-recipes")
        .exec()?;
    #[cfg(feature = "pulsar")]
    lua.load(PULSAR_RECIPES_LUA)
        .set_name("@prova/pulsar-recipes")
        .exec()?;
    #[cfg(feature = "kafka")]
    lua.load(KAFKA_RECIPES_LUA)
        .set_name("@prova/kafka-recipes")
        .exec()?;
    #[cfg(feature = "s3")]
    lua.load(S3_RECIPES_LUA)
        .set_name("@prova/s3-recipes")
        .exec()?;
    Ok(())
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
/// Spec fields: `name` (for messages), `image` (base repo), `tag` (default tag), `port`/`ports`
/// (published; `port` is the primary for readiness + url), `command?`, `env?` (table or
/// `function(opts)->table`), `wait?` (`{ port|log }`, default `{ port = primary }`), `timeout?`,
/// `url` (`function(host_port, opts)->string`, required), `client?`
/// (`function(url, opts, container)->handle` — the `container` is passed so a docker-exec client can
/// `exec` into it; a native client just uses `url`).
const CONTAINERIZED_LUA: &str = r#"
function prova.containerized(spec)
  assert(type(spec) == "table", "prova.containerized: pass a spec table")
  assert(spec.image and spec.url, "prova.containerized: spec needs `image` and `url`")
  local name = spec.name or "resource"
  local ports = spec.ports
  if type(ports) == "number" then ports = { ports } end
  ports = ports or { spec.port }
  local primary = spec.port or ports[1]
  assert(primary, "prova.containerized: spec needs a `port` (or `ports`)")

  local ns = { client = spec.client }

  function ns.container(ctx, opts)
    assert(ctx and ctx.manage, name .. ".container(ctx, opts?): pass the fixture/test context first")
    opts = opts or {}

    local image = opts.image
    if not image then
      image = spec.image
      local tag = opts.tag or spec.tag
      if tag then image = image .. ":" .. tag end
    end
    local timeout = opts.timeout or spec.timeout or "60s"

    local env = opts.env
    if env == nil then
      env = spec.env
      if type(env) == "function" then env = env(opts) end
    end

    local w = spec.wait or { port = primary }
    local wait = { port = w.port, log = w.log, timeout = timeout }

    local container = ctx:manage(docker.run{
      image = image, ports = ports, env = env, command = spec.command, wait = wait,
    })

    local url = spec.url(container:host_port(primary), opts)
    local res = { url = url, container = container }
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

/// `s3.container(ctx, opts?)` provisions an ephemeral MinIO (S3-compatible), creates a bucket, and
/// returns the standard resource shape `{ client, url, container }` plus `access_key`/`secret_key`.
/// Requires the `docker` module.
#[cfg(feature = "s3")]
const S3_RECIPES_LUA: &str = r#"
function s3.container(ctx, opts)
  assert(ctx and ctx.manage, "s3.container(ctx, opts?): pass the fixture/test context as the first argument")
  opts = opts or {}
  local image = opts.image or ("minio/minio:" .. (opts.tag or "latest"))
  local access = opts.access_key or "minioadmin"
  local secret = opts.secret_key or "minioadmin"
  local bucket_name = opts.bucket or "prova"
  local timeout = opts.timeout or "60s"
  local container = ctx:manage(docker.run{
    image = image,
    command = "server /data",
    ports = { 9000 },
    env = { MINIO_ROOT_USER = access, MINIO_ROOT_PASSWORD = secret },
    wait = { port = 9000, timeout = timeout },
  })
  local url = "http://127.0.0.1:" .. container:host_port(9000)
  -- Connecting with create=true hits MinIO (creating the bucket), so it doubles as the readiness gate.
  local client = ctx:manage(prova.retry(function()
    return s3.client{ url = url, access_key = access, secret_key = secret,
                      bucket = bucket_name, create = true }
  end, { timeout = timeout, message = "minio did not become ready in time" }))
  return { client = client, url = url, container = container,
           access_key = access, secret_key = secret }
end
"#;

/// `kafka.container(ctx, opts?)` provisions an ephemeral single-node Kafka (KRaft) and returns
/// `{ brokers, client, container }`. Unlike the others it uses a **fixed** host port (default 9092),
/// because Kafka advertises a listener address clients must be able to reach — so only one
/// `kafka.container` runs per host at a time. Requires the `docker` module at call time.
#[cfg(feature = "kafka")]
const KAFKA_RECIPES_LUA: &str = r#"
function kafka.container(ctx, opts)
  assert(ctx and ctx.manage, "kafka.container(ctx, opts?): pass the fixture/test context as the first argument")
  opts = opts or {}
  local image = opts.image or ("apache/kafka:" .. (opts.tag or "3.9.0"))
  local port = opts.port or 9092
  local timeout = opts.timeout or "90s"
  local container = ctx:manage(docker.run{
    image = image,
    ports = { { container = 9092, host = port } },
    env = {
      KAFKA_NODE_ID = "1",
      KAFKA_PROCESS_ROLES = "broker,controller",
      KAFKA_LISTENERS = "PLAINTEXT://:9092,CONTROLLER://:9093",
      KAFKA_ADVERTISED_LISTENERS = "PLAINTEXT://127.0.0.1:" .. port,
      KAFKA_CONTROLLER_QUORUM_VOTERS = "1@localhost:9093",
      KAFKA_CONTROLLER_LISTENER_NAMES = "CONTROLLER",
      KAFKA_LISTENER_SECURITY_PROTOCOL_MAP = "CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT",
      KAFKA_INTER_BROKER_LISTENER_NAME = "PLAINTEXT",
      KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR = "1",
      KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS = "0",
      KAFKA_TRANSACTION_STATE_LOG_MIN_ISR = "1",
      KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR = "1",
    },
    wait = { port = 9092, timeout = timeout },
  })
  local url = "127.0.0.1:" .. port
  local client = ctx:manage(prova.retry(function() return kafka.client(url) end,
    { timeout = timeout, message = "kafka did not accept connections in time" }))
  return { client = client, url = url, container = container }
end
"#;

/// `pulsar.container(ctx, opts?)` provisions an ephemeral Pulsar standalone, waits for it, connects,
/// and returns `{ url, client, container }`. Requires the `docker` module at call time. Pulsar
/// standalone is a heavy image and slow to start (tens of seconds); the default timeout reflects that.
#[cfg(feature = "pulsar")]
const PULSAR_RECIPES_LUA: &str = r#"
function pulsar.container(ctx, opts)
  assert(ctx and ctx.manage, "pulsar.container(ctx, opts?): pass the fixture/test context as the first argument")
  opts = opts or {}
  local image = opts.image or ("apachepulsar/pulsar:" .. (opts.tag or "3.3.1"))
  local timeout = opts.timeout or "120s"
  local container = ctx:manage(docker.run{
    image = image,
    command = "bin/pulsar standalone",
    ports = { 6650, 8080 },
    wait = { log = "messaging service is ready", timeout = timeout },
  })
  local url = "pulsar://127.0.0.1:" .. container:host_port(6650)
  local client = ctx:manage(prova.retry(function() return pulsar.client(url) end,
    { timeout = timeout, message = "pulsar did not accept connections in time" }))
  -- The "messaging service is ready" log (and a bare connect) are false-positives: the broker accepts
  -- connections before the public/default namespace bundle is loaded, so the first produce races it
  -- with "Namespace not found". Retry a real produce to a throwaway topic until it holds — the true
  -- readiness gate (same principle as the DB connect-retry), on a topic no test consumes.
  -- `produce` returns nothing on success, so return a truthy sentinel — prova.retry loops until the
  -- callback returns truthy (a raised "Namespace not found" counts as "not ready" and retries).
  prova.retry(function() client:produce("prova-readiness-probe", "ready"); return true end,
    { timeout = timeout, message = "pulsar namespace did not become ready in time" })
  return { client = client, url = url, container = container }
end
"#;

/// The Redis counterpart to `postgres.container`: `redis.container(ctx, opts?)` provisions an
/// ephemeral Redis, waits for it, opens a managed connection, and returns the standard resource
/// shape `{ client, url, container }`. Requires the `docker` module at call time.
#[cfg(feature = "redis")]
const REDIS_RECIPES_LUA: &str = r#"
-- Authored through prova.containerized — the same seam a third-party plugin uses (dogfood).
redis.container = prova.containerized{
  name = "redis", image = "redis", tag = "7-alpine", port = 6379,
  url = function(hp) return "redis://127.0.0.1:" .. hp end,
  client = function(url) return redis.client(url) end,
}.container
"#;

/// `postgres.container(ctx, opts?)` — a testcontainers-style recipe: one call provisions an
/// ephemeral Postgres, waits for it to actually accept connections, opens a managed connection, and
/// ties it all to the scope. Returns the standard resource shape `{ client, url, container }`.
/// Requires the `docker` module at call time and is `requires = { "docker" }`-gateable.
#[cfg(feature = "postgres")]
const POSTGRES_RECIPES_LUA: &str = r#"
-- Authored through prova.containerized (dogfood). `env`/`url` read `opts` for user/password/database.
-- The port opening is a false-positive for a first-boot DB (it restarts once at init); the client
-- factory + prova.retry (inside the helper) is the real readiness gate, doubling for anything wiring in.
postgres.container = prova.containerized{
  name = "postgres", image = "postgres", tag = "16-alpine", port = 5432,
  env = function(opts)
    return { POSTGRES_USER = opts.user or "prova", POSTGRES_PASSWORD = opts.password or "prova",
             POSTGRES_DB = opts.database or "prova" }
  end,
  url = function(hp, opts)
    return string.format("postgres://%s:%s@127.0.0.1:%d/%s",
      opts.user or "prova", opts.password or "prova", hp, opts.database or "prova")
  end,
  client = function(url) return postgres.client(url) end,
}.container
"#;

/// `mysql.container(ctx, opts?)` — the MySQL counterpart to `postgres.container`. Returns the
/// standard resource shape `{ client, url, container }`.
#[cfg(feature = "mysql")]
const MYSQL_RECIPES_LUA: &str = r#"
function mysql.container(ctx, opts)
  assert(ctx and ctx.manage, "mysql.container(ctx, opts?): pass the fixture/test context as the first argument")
  opts = opts or {}
  local user     = opts.user     or "prova"
  local password = opts.password or "prova"
  local database = opts.database or "prova"
  local image    = opts.image    or ("mysql:" .. (opts.tag or "8"))
  local timeout  = opts.timeout  or "90s"

  local container = ctx:manage(docker.run{
    image = image,
    env = {
      MYSQL_USER = user, MYSQL_PASSWORD = password, MYSQL_DATABASE = database,
      MYSQL_ROOT_PASSWORD = opts.root_password or "root",
    },
    ports = { 3306 },
    wait = { port = 3306, timeout = timeout },
  })

  local url = string.format("mysql://%s:%s@127.0.0.1:%d/%s", user, password, container:host_port(3306), database)
  -- The port opening is a false-positive for a first-boot DB (it restarts once at init); retry the
  -- real connection until it holds. This doubles as the readiness gate for anything else wiring in.
  local client = ctx:manage(prova.retry(function() return mysql.client(url) end,
    { timeout = timeout, message = "mysql did not accept connections in time" }))

  return { client = client, url = url, container = container }
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
}

impl UserData for Process {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("pid", |_, this| Ok(this.pid));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
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
        lua.create_async_function(|lua, (cmd, opts): (String, Option<Table>)| async move {
            // Extract options up front (owned) so nothing borrows Lua across the await.
            let cwd = opt_string(&opts, "cwd")?;
            let env = opt_env(&opts)?;
            let timeout = opt_string(&opts, "timeout")?.and_then(|s| parse_duration(&s));
            let check = opts
                .as_ref()
                .map(|o| o.get::<Option<bool>>("check"))
                .transpose()?
                .flatten()
                .unwrap_or(false);

            // Run the command string through a shell so `"cargo build --release"` works verbatim.
            let mut command = shell_command(&cmd);
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
                return Err(mlua::Error::RuntimeError(format!(
                    "shell.run: command exited {} (check=true): {cmd}\n{}",
                    result.code, result.stderr
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
        lua.create_function(|lua, (cmd, opts): (String, Option<Table>)| {
            let cwd = opt_string(&opts, "cwd")?;
            let env = opt_env(&opts)?;
            let mut command = shell_command(&cmd);
            if let Some(dir) = &cwd {
                command.current_dir(dir);
            }
            for (k, v) in &env {
                command.env(k, v);
            }
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let child = command
                .spawn()
                .map_err(|e| mlua::Error::RuntimeError(format!("shell.spawn failed: {e}")))?;
            let pid = child.id();
            lua.create_userdata(Process {
                child: Some(child),
                pid,
            })
        })?,
    )?;

    Ok(shell)
}

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
            for pair in env.pairs::<String, String>() {
                let (k, v) = pair?;
                out.push((k, v));
            }
        }
    }
    Ok(out)
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
        methods.add_async_method(name, move |lua, this, (path, opts): (String, Option<Table>)| {
            let url = join_url(&this.base_url, &path);
            let prepared =
                build_prepared(&lua, method.clone(), url, this.headers.clone(), this.timeout, opts);
            async move {
                let resp = send(prepared?).await?;
                lua.create_userdata(resp)
            }
        });
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
                upsert_header(&mut headers, "content-type".into(), "application/json".into());
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
        match headers.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
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
// docker (testcontainers-style ephemeral dependencies, via the typed bollard daemon client)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "docker")]
mod docker {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use bollard::container::{
        Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
        StartContainerOptions,
    };
    use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
    use bollard::image::CreateImageOptions;
    use bollard::models::{HostConfig, PortBinding};
    use bollard::Docker;
    use futures::StreamExt;
    use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

    use crate::model::parse_duration;

    /// A running container from `docker.run` — same Lua surface as before, now backed by the typed
    /// bollard daemon client (structured errors, streamed logs/exec, no CLI parsing). `c.id`,
    /// `c:host_port(p)`, `c:endpoint(p)`, async `c:logs()`, `c:exec(cmd)`, `c:stop()`. `:stop`
    /// force-removes; a `Drop` backstop removes it if a test forgot to. Blessed pattern:
    /// `ctx:defer(function() c:stop() end)`.
    struct Container {
        client: Docker,
        id: String,
        ports: HashMap<u16, u16>, // container port -> mapped host port
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
            methods.add_method("host_port", |_, this, port: u16| {
                this.ports.get(&port).copied().ok_or_else(|| {
                    mlua::Error::RuntimeError(format!("container port {port} was not published"))
                })
            });
            methods.add_method("endpoint", |_, this, port: u16| {
                this.ports
                    .get(&port)
                    .map(|hp| format!("127.0.0.1:{hp}"))
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "container port {port} was not published"
                        ))
                    })
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

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let docker = lua.create_table()?;
        docker.set("run", run_fn(lua)?)?;
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
                mlua::Value::Table(t) => t.sequence_values::<String>().collect::<mlua::Result<_>>()?,
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
            Ok(Spec {
                image,
                ports,
                env,
                command,
                wait,
            })
        }
    }

    fn derr<E: std::fmt::Display>(e: E) -> mlua::Error {
        mlua::Error::RuntimeError(format!("docker: {e}"))
    }

    async fn start(spec: Spec) -> mlua::Result<Container> {
        let client = Docker::connect_with_local_defaults().map_err(derr)?;

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
                    host_port: Some(host.map(|h| h.to_string()).unwrap_or_else(|| "0".to_string())),
                }]),
            );
        }

        let config = Config {
            image: Some(spec.image.clone()),
            env: Some(spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
            cmd: (!spec.command.is_empty()).then(|| spec.command.clone()),
            exposed_ports: (!exposed.is_empty()).then_some(exposed),
            host_config: Some(HostConfig {
                port_bindings: (!bindings.is_empty()).then_some(bindings),
                ..Default::default()
            }),
            ..Default::default()
        };

        let created = client
            .create_container(None::<CreateContainerOptions<String>>, config)
            .await
            .map_err(derr)?;
        let id = created.id;
        client
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .map_err(derr)?;

        let mut container = Container {
            client: client.clone(),
            id: id.clone(),
            ports: HashMap::new(),
            stopped: false,
        };

        // Inspect for the assigned host ports.
        let info = client.inspect_container(&id, None).await.map_err(derr)?;
        if let Some(ports) = info.network_settings.and_then(|ns| ns.ports) {
            for (container_port, _) in &spec.ports {
                if let Some(Some(binds)) = ports.get(&format!("{container_port}/tcp")) {
                    if let Some(hp) = binds
                        .first()
                        .and_then(|b| b.host_port.as_ref())
                        .and_then(|s| s.parse::<u16>().ok())
                    {
                        container.ports.insert(*container_port, hp);
                    }
                }
            }
        }

        if let Some(wait) = spec.wait {
            wait_ready(&container, &wait).await?;
        }
        Ok(container)
    }

    async fn wait_ready(container: &Container, wait: &Wait) -> mlua::Result<()> {
        let deadline = Instant::now() + wait.timeout;
        loop {
            let ready = if let Some(port) = wait.port {
                match container.ports.get(&port) {
                    Some(&host_port) => tokio::net::TcpStream::connect(("127.0.0.1", host_port))
                        .await
                        .is_ok(),
                    None => false,
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
            if Instant::now() >= deadline {
                return Err(mlua::Error::RuntimeError(format!(
                    "docker.run: container {} not ready within {:?}",
                    container.id, wait.timeout
                )));
            }
            tokio::time::sleep(wait.every).await;
        }
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
        if let StartExecResults::Attached { mut output, mut input } = client
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
}

// ---------------------------------------------------------------------------------------------
// sql (postgres/mysql/sqlite namespaces over one generic Connection via sqlx's `Any` driver)
// ---------------------------------------------------------------------------------------------

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod sql {
    use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
    use sqlx::any::{AnyPoolOptions, AnyRow, AnyTypeInfoKind};
    use sqlx::{AnyPool, Column, Row};

    /// Which SQL engine a namespace fronts. Every engine's `client(url)` returns the same generic
    /// `Connection` (sqlx `Any` driver) — the namespace exists for discoverability and URL-scheme
    /// validation, not for a per-engine API.
    #[derive(Clone, Copy)]
    pub(crate) enum Engine {
        Postgres,
        Mysql,
        Sqlite,
    }

    impl Engine {
        fn name(self) -> &'static str {
            match self {
                Engine::Postgres => "postgres",
                Engine::Mysql => "mysql",
                Engine::Sqlite => "sqlite",
            }
        }
        fn schemes(self) -> &'static [&'static str] {
            match self {
                Engine::Postgres => &["postgres://", "postgresql://"],
                Engine::Mysql => &["mysql://"],
                Engine::Sqlite => &["sqlite://", "sqlite:"],
            }
        }
    }

    /// A database connection pool from `postgres.client(url)` / `mysql.client(url)` /
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
        method.ok_or_else(|| err(format!("grpc: method {method_name:?} not found on {service}")))
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
            output: desc.output(),
        };
        let mut request = Request::new(input);
        if let Some(t) = timeout {
            request.set_timeout(t);
        }
        let resp = grpc.unary(request, path, codec).await?;
        Ok(resp.into_inner())
    }

    // A tonic codec that speaks `DynamicMessage` on both ends: the encoder just prost-encodes the
    // request; the decoder builds an empty message of the known output type and merges the reply
    // bytes into it. This is the whole trick that lets one client call any method dynamically.
    #[derive(Clone)]
    struct DynCodec {
        output: MessageDescriptor,
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
                output: self.output.clone(),
            }
        }
    }

    struct DynEncoder;
    impl Encoder for DynEncoder {
        type Item = DynamicMessage;
        type Error = Status;
        fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
            item.encode(dst)
                .map_err(|e| Status::internal(format!("grpc: encoding request: {e}")))
        }
    }

    struct DynDecoder {
        output: MessageDescriptor,
    }
    impl Decoder for DynDecoder {
        type Item = DynamicMessage;
        type Error = Status;
        fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
            let mut msg = DynamicMessage::new(self.output.clone());
            msg.merge(src)
                .map_err(|e| Status::internal(format!("grpc: decoding response: {e}")))?;
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
            let raw = files_for_symbol(channel, rv, service)
                .await
                .map_err(|e| err(format!("grpc: reflecting {service}: {} ({})", e.message(), e.code())))?;
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
                            return Err(Status::new(tonic::Code::from(e.error_code), e.error_message));
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
                            return Err(Status::new(tonic::Code::from(e.error_code), e.error_message));
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
            Some(t) => t.get::<Option<String>>(key)?.and_then(|s| parse_duration(&s)),
            None => None,
        })
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
                        mlua::Error::RuntimeError(format!("yaml.parse_all: document {}: {e}", i + 1))
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
                        let data = json
                            .get("data")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
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
                            json_to_lua(&lua, &json.get("data").cloned().unwrap_or(serde_json::Value::Null))?,
                        )?;
                        out.set(
                            "errors",
                            json_to_lua(&lua, &json.get("errors").cloned().unwrap_or(serde_json::Value::Null))?,
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

// ---------------------------------------------------------------------------------------------
// redis (async; a thin cache client — get/set/del/exists/incr/ping + a generic command)
// ---------------------------------------------------------------------------------------------

// Enough to assert on a cache dependency: check a key the app set, seed a value, count keys. The
// generic `:command(...)` is the escape hatch for anything not covered. No TLS in v1 (local/CI).
#[cfg(feature = "redis")]
mod redis_mod {
    use mlua::{Lua, Table, UserData, UserDataMethods, Value, Variadic};

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    /// A Redis connection from `redis.client`. `MultiplexedConnection` is a cheap cloneable handle,
    /// so each async method clones it into its future (nothing borrows Lua across the await).
    struct RedisConnection {
        conn: redis::aio::MultiplexedConnection,
    }

    /// Convert a raw Redis reply to Lua (for the generic `command`); typed methods use redis's own
    /// `FromRedisValue` conversion instead.
    fn value_to_lua(lua: &Lua, v: redis::Value) -> mlua::Result<Value> {
        Ok(match v {
            redis::Value::Nil => Value::Nil,
            redis::Value::Int(i) => Value::Integer(i),
            redis::Value::Double(d) => Value::Number(d),
            redis::Value::Boolean(b) => Value::Boolean(b),
            redis::Value::BulkString(bytes) => Value::String(lua.create_string(bytes)?),
            redis::Value::SimpleString(s) => Value::String(lua.create_string(s)?),
            redis::Value::Okay => Value::String(lua.create_string("OK")?),
            redis::Value::Array(items) | redis::Value::Set(items) => {
                let t = lua.create_table()?;
                for item in items {
                    t.push(value_to_lua(lua, item)?)?;
                }
                Value::Table(t)
            }
            redis::Value::Map(pairs) => {
                let t = lua.create_table()?;
                for (k, val) in pairs {
                    let key = value_to_lua(lua, k)?;
                    t.set(key, value_to_lua(lua, val)?)?;
                }
                Value::Table(t)
            }
            other => Value::String(lua.create_string(format!("{other:?}"))?),
        })
    }

    impl UserData for RedisConnection {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // get(key) → string | nil
            methods.add_async_method("get", |_, this, key: String| {
                let mut conn = this.conn.clone();
                async move {
                    redis::cmd("GET")
                        .arg(key)
                        .query_async::<Option<String>>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis GET: {e}")))
                }
            });
            // set(key, value)
            methods.add_async_method("set", |_, this, (key, value): (String, String)| {
                let mut conn = this.conn.clone();
                async move {
                    redis::cmd("SET")
                        .arg(key)
                        .arg(value)
                        .query_async::<()>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis SET: {e}")))
                }
            });
            // del(key, ...) → number of keys removed
            methods.add_async_method("del", |_, this, keys: Variadic<String>| {
                let mut conn = this.conn.clone();
                let keys: Vec<String> = keys.into_iter().collect();
                async move {
                    redis::cmd("DEL")
                        .arg(keys)
                        .query_async::<i64>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis DEL: {e}")))
                }
            });
            // exists(key) → bool
            methods.add_async_method("exists", |_, this, key: String| {
                let mut conn = this.conn.clone();
                async move {
                    let n: i64 = redis::cmd("EXISTS")
                        .arg(key)
                        .query_async(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis EXISTS: {e}")))?;
                    Ok(n > 0)
                }
            });
            // incr(key, by?) → new value
            methods.add_async_method("incr", |_, this, (key, by): (String, Option<i64>)| {
                let mut conn = this.conn.clone();
                async move {
                    redis::cmd("INCRBY")
                        .arg(key)
                        .arg(by.unwrap_or(1))
                        .query_async::<i64>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis INCRBY: {e}")))
                }
            });
            // expire(key, seconds)
            methods.add_async_method("expire", |_, this, (key, seconds): (String, i64)| {
                let mut conn = this.conn.clone();
                async move {
                    redis::cmd("EXPIRE")
                        .arg(key)
                        .arg(seconds)
                        .query_async::<()>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis EXPIRE: {e}")))
                }
            });
            // ping() → "PONG"
            methods.add_async_method("ping", |_, this, ()| {
                let mut conn = this.conn.clone();
                async move {
                    redis::cmd("PING")
                        .query_async::<String>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis PING: {e}")))
                }
            });
            // command(name, args...) → the raw reply as a Lua value (the escape hatch)
            methods.add_async_method("command", |lua, this, args: Variadic<String>| {
                let mut conn = this.conn.clone();
                let args: Vec<String> = args.into_iter().collect();
                async move {
                    let mut it = args.into_iter();
                    let name = it
                        .next()
                        .ok_or_else(|| err("redis command: needs at least a command name"))?;
                    let mut cmd = redis::cmd(&name);
                    for a in it {
                        cmd.arg(a);
                    }
                    let v = cmd
                        .query_async::<redis::Value>(&mut conn)
                        .await
                        .map_err(|e| err(format!("redis {name}: {e}")))?;
                    value_to_lua(&lua, v)
                }
            });
            // close() — a no-op (the multiplexed handle drops with the userdata); present so a redis
            // connection is `ctx:manage`-able for symmetry with the SQL clients.
            methods.add_method("close", |_, _this, ()| Ok(()));
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let redis_tbl = lua.create_table()?;
        // redis.client(url) → a Connection. Async (needs the runtime); call in a fixture/test body.
        redis_tbl.set(
            "client",
            lua.create_async_function(|lua, url: String| async move {
                let client =
                    redis::Client::open(url).map_err(|e| err(format!("redis.client: {e}")))?;
                let conn = client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(|e| err(format!("redis.client: {e}")))?;
                lua.create_userdata(RedisConnection { conn })
            })?,
        )?;
        Ok(redis_tbl)
    }
}

// ---------------------------------------------------------------------------------------------
// pulsar (async; a thin produce/consume client for asserting on a messaging dependency)
// ---------------------------------------------------------------------------------------------

// Enough to drive a messaging dependency from a test: produce a message an app should consume, or
// consume messages an app produced and assert on them. Consumers read from the earliest offset so a
// produce-then-consume within a test is reliable regardless of ordering. Plaintext only in v1 (no
// TLS/token auth — local/CI brokers; an environment/TLS layer lands later).
#[cfg(feature = "pulsar")]
mod pulsar_mod {
    use std::time::{Duration, Instant};

    use futures::StreamExt;
    use mlua::{Lua, Table, UserData, UserDataMethods};
    use pulsar::consumer::InitialPosition;
    use pulsar::{Consumer, ConsumerOptions, Pulsar, SubType, TokioExecutor};

    use crate::model::parse_duration;

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    /// A Pulsar client from `pulsar.client`. `Pulsar<TokioExecutor>` is a cheap cloneable handle.
    struct PulsarClient {
        client: Pulsar<TokioExecutor>,
    }

    impl UserData for PulsarClient {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // produce(topic, message) — send a string and await the broker's receipt (confirmed send).
            methods.add_async_method("produce", |_, this, (topic, message): (String, String)| {
                let client = this.client.clone();
                async move {
                    let receipt = client
                        .send(topic, message)
                        .await
                        .map_err(|e| err(format!("pulsar produce: {e}")))?;
                    receipt
                        .await
                        .map_err(|e| err(format!("pulsar produce (receipt): {e}")))?;
                    Ok(())
                }
            });

            // consume(topic, { subscription?, max?, timeout?, shared? }) → list of message strings.
            // Reads from the earliest offset; collects up to `max` messages arriving within `timeout`.
            methods.add_async_method(
                "consume",
                |lua, this, (topic, opts): (String, Option<Table>)| {
                    let client = this.client.clone();
                    // Parse opts synchronously (nothing borrows Lua across the await).
                    let mut subscription = "prova".to_string();
                    let mut max = 10usize;
                    let mut timeout = Duration::from_secs(10);
                    let mut shared = false;
                    let parsed = (|| -> mlua::Result<()> {
                        if let Some(o) = &opts {
                            if let Some(s) = o.get::<Option<String>>("subscription")? {
                                subscription = s;
                            }
                            if let Some(m) = o.get::<Option<usize>>("max")? {
                                max = m;
                            }
                            if let Some(t) = o
                                .get::<Option<String>>("timeout")?
                                .and_then(|s| parse_duration(&s))
                            {
                                timeout = t;
                            }
                            if let Some(b) = o.get::<Option<bool>>("shared")? {
                                shared = b;
                            }
                        }
                        Ok(())
                    })();
                    async move {
                        parsed?;
                        let sub_type = if shared {
                            SubType::Shared
                        } else {
                            SubType::Exclusive
                        };
                        let mut consumer: Consumer<Vec<u8>, TokioExecutor> = client
                            .consumer()
                            .with_topic(&topic)
                            .with_subscription(&subscription)
                            .with_subscription_type(sub_type)
                            .with_options(
                                ConsumerOptions::default()
                                    .with_initial_position(InitialPosition::Earliest),
                            )
                            .build()
                            .await
                            .map_err(|e| err(format!("pulsar consume (subscribe): {e}")))?;

                        let deadline = Instant::now() + timeout;
                        let out = lua.create_table()?;
                        let mut n = 0usize;
                        while n < max {
                            let remaining = deadline.saturating_duration_since(Instant::now());
                            if remaining.is_zero() {
                                break;
                            }
                            match tokio::time::timeout(remaining, consumer.next()).await {
                                Ok(Some(Ok(msg))) => {
                                    let s = String::from_utf8_lossy(&msg.payload.data).into_owned();
                                    out.push(s)?;
                                    n += 1;
                                    let _ = consumer.ack(&msg).await;
                                }
                                Ok(Some(Err(e))) => {
                                    return Err(err(format!("pulsar consume: {e}")))
                                }
                                Ok(None) => break, // stream ended
                                Err(_) => break,   // no more messages within the window
                            }
                        }
                        Ok(out)
                    }
                },
            );

            // close() — a no-op (the client drops with the handle); present for `ctx:manage` symmetry.
            methods.add_method("close", |_, _this, ()| Ok(()));
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let pulsar_tbl = lua.create_table()?;
        // pulsar.client(url) → a client. Async (connects to the broker); call in a fixture/test body.
        pulsar_tbl.set(
            "client",
            lua.create_async_function(|lua, url: String| async move {
                let client = Pulsar::builder(url, TokioExecutor)
                    .build()
                    .await
                    .map_err(|e| err(format!("pulsar.client: {e}")))?;
                lua.create_userdata(PulsarClient { client })
            })?,
        )?;
        Ok(pulsar_tbl)
    }
}

// ---------------------------------------------------------------------------------------------
// kafka (async; a thin produce/consume client via rdkafka — librdkafka statically linked)
// ---------------------------------------------------------------------------------------------

// The Kafka counterpart to the pulsar module: produce a message an app should consume, or consume
// messages an app produced and assert on them. Consumers use a fresh group with auto-commit off and
// `auto.offset.reset=earliest`, so produce-then-consume within one test reads from the start.
// Plaintext only in v1 (no SSL/SASL — so no openssl either).
#[cfg(feature = "kafka")]
mod kafka_mod {
    use std::time::{Duration, Instant};

    use mlua::{Lua, Table, UserData, UserDataMethods};
    use rdkafka::config::ClientConfig;
    use rdkafka::consumer::{Consumer, StreamConsumer};
    use rdkafka::message::Message;
    use rdkafka::producer::{FutureProducer, FutureRecord, Producer};
    use rdkafka::util::Timeout;

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    /// A Kafka client bound to a set of bootstrap brokers. Holds a shared producer; consumers are
    /// created per `consume` call (each needs its own group/subscription config).
    struct KafkaClient {
        brokers: String,
        producer: FutureProducer,
    }

    impl UserData for KafkaClient {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // produce(topic, message) — send a string and await the broker's delivery ack.
            methods.add_async_method("produce", |_, this, (topic, message): (String, String)| {
                let producer = this.producer.clone();
                async move {
                    let record = FutureRecord::to(&topic).payload(&message).key("");
                    producer
                        .send(record, Timeout::After(Duration::from_secs(10)))
                        .await
                        .map_err(|(e, _)| err(format!("kafka produce: {e}")))?;
                    Ok(())
                }
            });

            // consume(topic, { group?, max?, timeout? }) → list of message strings, from the earliest
            // offset. Collects up to `max` messages arriving within `timeout`.
            methods.add_async_method(
                "consume",
                |lua, this, (topic, opts): (String, Option<Table>)| {
                    let brokers = this.brokers.clone();
                    let mut group = "prova".to_string();
                    let mut max = 10usize;
                    let mut timeout = Duration::from_secs(15);
                    let parsed = (|| -> mlua::Result<()> {
                        if let Some(o) = &opts {
                            if let Some(g) = o.get::<Option<String>>("group")? {
                                group = g;
                            }
                            if let Some(m) = o.get::<Option<usize>>("max")? {
                                max = m;
                            }
                            if let Some(t) = o
                                .get::<Option<String>>("timeout")?
                                .and_then(|s| crate::model::parse_duration(&s))
                            {
                                timeout = t;
                            }
                        }
                        Ok(())
                    })();
                    async move {
                        parsed?;
                        let consumer: StreamConsumer = ClientConfig::new()
                            .set("bootstrap.servers", &brokers)
                            .set("group.id", &group)
                            .set("auto.offset.reset", "earliest")
                            .set("enable.auto.commit", "false")
                            .set("session.timeout.ms", "6000")
                            .create()
                            .map_err(|e| err(format!("kafka consumer: {e}")))?;
                        consumer
                            .subscribe(&[&topic])
                            .map_err(|e| err(format!("kafka subscribe {topic}: {e}")))?;

                        let deadline = Instant::now() + timeout;
                        let out = lua.create_table()?;
                        let mut n = 0usize;
                        while n < max {
                            let remaining = deadline.saturating_duration_since(Instant::now());
                            if remaining.is_zero() {
                                break;
                            }
                            match tokio::time::timeout(remaining, consumer.recv()).await {
                                Ok(Ok(msg)) => {
                                    let payload = msg
                                        .payload()
                                        .map(|b| String::from_utf8_lossy(b).into_owned())
                                        .unwrap_or_default();
                                    out.push(payload)?;
                                    n += 1;
                                }
                                Ok(Err(e)) => return Err(err(format!("kafka consume: {e}"))),
                                Err(_) => break, // no more messages within the window
                            }
                        }
                        Ok(out)
                    }
                },
            );

            methods.add_method("close", |_, _this, ()| Ok(()));
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let kafka = lua.create_table()?;
        // kafka.client(brokers) → a client. Async: creates a producer and verifies connectivity with
        // a metadata fetch (so `prova.retry(kafka.client, ...)` is a real readiness gate).
        kafka.set(
            "client",
            lua.create_async_function(|lua, brokers: String| async move {
                let producer: FutureProducer = ClientConfig::new()
                    .set("bootstrap.servers", &brokers)
                    .set("message.timeout.ms", "10000")
                    .create()
                    .map_err(|e| err(format!("kafka.client: {e}")))?;
                // fetch_metadata is blocking; run it on the blocking pool so the worker isn't stalled.
                let probe = producer.clone();
                let brokers_for_probe = brokers.clone();
                tokio::task::spawn_blocking(move || {
                    probe
                        .client()
                        .fetch_metadata(None, Timeout::After(Duration::from_secs(5)))
                })
                .await
                .map_err(|e| err(format!("kafka.client: {e}")))?
                .map_err(|e| err(format!("kafka.client {brokers_for_probe}: {e}")))?;
                lua.create_userdata(KafkaClient { brokers, producer })
            })?,
        )?;
        Ok(kafka)
    }
}

// ---------------------------------------------------------------------------------------------
// s3 (async; a thin object-storage client — put/get/exists/list/delete against S3/MinIO)
// ---------------------------------------------------------------------------------------------

// Enough to assert on an object-storage dependency: an object an app wrote, its contents, a listing.
// rust-s3 with rustls (pure-Rust, statically linked — no openssl). Path-style addressing (MinIO).
#[cfg(feature = "s3")]
mod s3_mod {
    use mlua::{Lua, Table, UserData, UserDataMethods};
    use s3::bucket::Bucket;
    use s3::creds::Credentials;
    use s3::region::Region;
    use s3::BucketConfiguration;

    fn err(msg: impl Into<String>) -> mlua::Error {
        mlua::Error::RuntimeError(msg.into())
    }

    /// A client bound to one S3/MinIO bucket. `Bucket` is a cheap cloneable handle.
    struct S3Bucket {
        bucket: Bucket,
    }

    impl UserData for S3Bucket {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            // put(key, data) — write an object (bytes of the string).
            methods.add_async_method("put", |_, this, (key, data): (String, String)| {
                let bucket = this.bucket.clone();
                async move {
                    bucket
                        .put_object(&key, data.as_bytes())
                        .await
                        .map_err(|e| err(format!("s3 put {key}: {e}")))?;
                    Ok(())
                }
            });
            // get(key) → the object's contents as a string (raises if it does not exist).
            methods.add_async_method("get", |_, this, key: String| {
                let bucket = this.bucket.clone();
                async move {
                    let resp = bucket
                        .get_object(&key)
                        .await
                        .map_err(|e| err(format!("s3 get {key}: {e}")))?;
                    if resp.status_code() >= 300 {
                        return Err(err(format!("s3 get {key}: status {}", resp.status_code())));
                    }
                    resp.to_string()
                        .map_err(|e| err(format!("s3 get {key}: {e}")))
                }
            });
            // exists(key) → bool (via a HEAD).
            methods.add_async_method("exists", |_, this, key: String| {
                let bucket = this.bucket.clone();
                async move {
                    match bucket.head_object(&key).await {
                        Ok((_, status)) => Ok(status == 200),
                        Err(_) => Ok(false),
                    }
                }
            });
            // delete(key)
            methods.add_async_method("delete", |_, this, key: String| {
                let bucket = this.bucket.clone();
                async move {
                    bucket
                        .delete_object(&key)
                        .await
                        .map_err(|e| err(format!("s3 delete {key}: {e}")))?;
                    Ok(())
                }
            });
            // list(prefix?) → the keys under the (optional) prefix.
            methods.add_async_method("list", |lua, this, prefix: Option<String>| {
                let bucket = this.bucket.clone();
                async move {
                    let results = bucket
                        .list(prefix.unwrap_or_default(), None)
                        .await
                        .map_err(|e| err(format!("s3 list: {e}")))?;
                    let out = lua.create_table()?;
                    for page in results {
                        for object in page.contents {
                            out.push(object.key)?;
                        }
                    }
                    Ok(out)
                }
            });
            methods.add_method("close", |_, _this, ()| Ok(()));
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let s3 = lua.create_table()?;
        // s3.client{ url, access_key, secret_key, bucket, region?, create? } → a bucket client.
        // Async; with `create = true` it creates the bucket (idempotent) — the recipe uses that as the
        // readiness gate. Call in a fixture/test body.
        s3.set(
            "client",
            lua.create_async_function(|lua, opts: Table| async move {
                let endpoint: String = opts
                    .get::<Option<String>>("url")?
                    .ok_or_else(|| err("s3.client requires a `url` (the endpoint, e.g. \"http://127.0.0.1:9000\")"))?;
                let bucket_name: String = opts
                    .get::<Option<String>>("bucket")?
                    .ok_or_else(|| err("s3.client requires a `bucket`"))?;
                let access: String = opts.get::<Option<String>>("access_key")?.unwrap_or_default();
                let secret: String = opts.get::<Option<String>>("secret_key")?.unwrap_or_default();
                let region_name = opts
                    .get::<Option<String>>("region")?
                    .unwrap_or_else(|| "us-east-1".to_string());
                let create = opts.get::<Option<bool>>("create")?.unwrap_or(false);

                let region = Region::Custom {
                    region: region_name,
                    endpoint,
                };
                let creds = Credentials::new(Some(&access), Some(&secret), None, None, None)
                    .map_err(|e| err(format!("s3.client credentials: {e}")))?;

                if create {
                    // Hits the network (readiness); tolerate "already exists".
                    if let Err(e) = Bucket::create_with_path_style(
                        &bucket_name,
                        region.clone(),
                        creds.clone(),
                        BucketConfiguration::default(),
                    )
                    .await
                    {
                        let msg = e.to_string();
                        if !msg.contains("BucketAlreadyOwnedByYou")
                            && !msg.contains("BucketAlreadyExists")
                            && !msg.to_lowercase().contains("already")
                        {
                            return Err(err(format!("s3.client create bucket: {e}")));
                        }
                    }
                }

                let bucket = *Bucket::new(&bucket_name, region, creds)
                    .map_err(|e| err(format!("s3.client: {e}")))?
                    .with_path_style();
                lua.create_userdata(S3Bucket { bucket })
            })?,
        )?;
        Ok(s3)
    }
}
