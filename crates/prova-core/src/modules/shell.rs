//! The `shell` module (`shell.run` / `shell.spawn` — async child processes) and the
//! `prova.containerized` scaffolding recipe it loads alongside.

use std::sync::Arc;
use std::time::Instant;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::model::parse_duration;
use crate::progress::{self, Kind, Progress};

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
/// `{ context, dockerfile?, tag?, buildargs?, secrets?, target?, pull?, nocache? }` table, or a bare string as
/// shorthand for `{ context = … }`), `tag` (default tag; pulled images only), `port`/`ports`
/// (published; `port` is the primary for readiness + url; a `ports` entry may be a number for a
/// random host port or `{ container, host }` for a fixed one), `command?`, `env?` (table or
/// `function(opts)->table`), `wait?` (`{ port|log }`, default `{ port = primary }`), `timeout?`,
/// `url` (`function(host_port, opts)->string`, required), `client?`
/// (`function(url, opts, container)->handle` — the `container` is passed so a docker-exec client can
/// `exec` into it; a native client just uses `url`), `extra?` (`function(url, opts, container)->table`
/// of additional resource fields beyond the trio, e.g. s3 credentials).
pub(crate) const CONTAINERIZED_LUA: &str = r#"
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

    -- The tap (docs/design/mocks-proxies-drivers.md): interpose a `socket.proxy` between the SUT
    -- and the REAL container, so any resource gets transcripts and the fault vocabulary with zero
    -- protocol knowledge — the payoff the L4 wiretap was built for. Readiness already probed the
    -- real container port above; the tap is added after, so it never makes a dead resource look
    -- alive. `tap = true` uses the recipe's declared `framing` (turn-level transcripts); a table
    -- overrides it (`tap = { framing = … }`); no framing = raw chunk-level.
    if opts.tap then
      local framing = spec.framing
      if type(opts.tap) == "table" and opts.tap.framing ~= nil then framing = opts.tap.framing end
      local tap = socket.proxy(ctx, {
        upstream = "tcp://127.0.0.1:" .. hp,
        framing = framing,
      })
      -- Route the SUT through the proxy: swap the real host:port authority for the tap's. Works
      -- for a tcp:// url and for a scheme'd one (redis://, amqp://) alike — it is authority
      -- substitution, not a scheme rewrite.
      local tap_hp = tap.addr:match("tcp://127%.0%.0%.1:(%d+)")
      res.url = res.url:gsub("127%.0%.0%.1:" .. hp, "127.0.0.1:" .. tap_hp)
      res.host, res.port = "127.0.0.1", tonumber(tap_hp)
      res.tap = tap
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
        // Kill and reap. Idempotent — a second stop, or stop after exit, is a no-op.
        methods.add_async_method_mut("stop", |_, mut this, ()| async move {
            if let Some(mut child) = this.child.take() {
                // On Windows the command runs under `cmd /C <cmd>`, which does NOT exec-replace: cmd.exe
                // stays the parent and the real program (e.g. python) is a CHILD that survives
                // TerminateProcess(cmd.exe) and keeps the inherited stdout/stderr pipe open — wedging
                // teardown forever on the never-closing pipe. Kill the whole tree so descendants die
                // and the pipes reach EOF. (On unix, `sh -c <one command>` exec-replaces, so the child
                // IS the program and `child.kill()` alone suffices.)
                #[cfg(windows)]
                if let Some(pid) = this.pid {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .output();
                }
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

pub(crate) fn make_shell(lua: &Lua, progress: &Arc<dyn Progress>) -> mlua::Result<Table> {
    let shell = lua.create_table()?;
    // The process transport's interpose posture: the journaling PATH shim (proofs/spec/process).
    shell.set("proxy", super::shellproxy::proxy_fn(lua)?)?;
    shell.set(
        "run",
        {
            let progress = Arc::clone(progress);
            lua.create_async_function(move |lua, (cmd, opts): (mlua::Value, Option<Table>)| {
                let progress = Arc::clone(&progress);
                async move {
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
            // Fold stderr into stdout in the result — the portable replacement for the `2>&1` redirect.
            let merge_stderr = opts
                .as_ref()
                .map(|o| o.get::<Option<bool>>("merge_stderr"))
                .transpose()?
                .flatten()
                .unwrap_or(false);
            // Feed the program's stdin — the portable replacement for a `printf x | cmd` pipe.
            let stdin = opt_string(&opts, "stdin")?;

            // A string runs through a shell (`"cargo build --release"` verbatim); an argv table runs
            // the program directly — no shell, no quoting.
            let mut command = cmd.build();
            if let Some(dir) = &cwd {
                command.current_dir(dir);
            }
            for (k, v) in &env {
                command.env(k, v);
            }

            // Bracket the blocking region: a captured build says nothing until it exits, which is
            // pause #2 in the inventory. The renderer decides whether this is worth a line — a 30ms
            // `echo` stays silent, a two-minute `cargo build` does not.
            let activity = progress::start(&progress, Kind::Command, cmd.display_name());
            let start = Instant::now();
            // With `stdin`, pipe the input in and reap via wait_with_output; otherwise `output()`
            // with stdin EXPLICITLY nulled. tokio's `output()` — unlike std's — only forces
            // stdout/stderr and lets stdin INHERIT, so a child that reads stdin (a journaling
            // shim's `cat`, an interactive prompt) blocks forever whenever the harness's own
            // stdin is a non-closing pipe. That was a live 40-minute suite hang under the
            // coverage conduct. Hermetic default: a test's child sees EOF, never the harness's
            // stdin; a proof that means to feed input says `stdin = ...`.
            let run = async {
                if let Some(input) = &stdin {
                    use tokio::io::AsyncWriteExt;
                    command
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    let mut child = command.spawn()?;
                    if let Some(mut si) = child.stdin.take() {
                        si.write_all(input.as_bytes()).await?;
                        si.shutdown().await?; // close so the child sees EOF
                    }
                    child.wait_with_output().await
                } else {
                    command.stdin(std::process::Stdio::null());
                    command.output().await
                }
            };
            let output = match timeout {
                Some(budget) => tokio::time::timeout(budget, run).await.map_err(|_| {
                    mlua::Error::RuntimeError(format!(
                        "shell.run timed out after {budget:?}: {cmd}"
                    ))
                })?,
                None => run.await,
            }
            .map_err(|e| mlua::Error::RuntimeError(format!("shell.run failed to spawn: {e}")))?;

            let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if merge_stderr {
                // Post-hoc concatenation: the streams were captured on separate pipes, so exact
                // interleaving is approximate, but all output is present in `stdout` and `stderr` is
                // emptied — the `2>&1` intent (everything on one stream) without a shell.
                stdout.push_str(&stderr);
                stderr.clear();
            }
            let result = ShellResult {
                code: output.status.code().unwrap_or(-1),
                stdout,
                stderr,
                duration: start.elapsed().as_secs_f64(),
            };
            if result.code == 0 {
                activity.done();
            } else {
                activity.done_with(format!("exit {}", result.code));
            }
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
                }
            })?
        },
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
                // stdin nulled for the same hermeticity as shell.run: a spawned server must
                // never sit on the HARNESS's stdin (tokio inherits it by default).
                .stdin(std::process::Stdio::null())
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
    /// A short label for an activity line. Truncated hard: a `cargo build` invocation with twenty
    /// flags is not what someone staring at a stalled run needs — the program and a hint of its
    /// arguments is. The full command is still in the error on failure.
    fn display_name(&self) -> String {
        const MAX: usize = 60;
        let full = match self {
            Self::Shell(s) => s.clone(),
            Self::Argv(argv) => argv.join(" "),
        };
        let flat = full.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= MAX {
            return flat;
        }
        let head: String = flat.chars().take(MAX - 1).collect();
        format!("{head}…")
    }

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
        let mut c = match self {
            Self::Shell(s) => shell_command(s),
            Self::Argv(argv) => {
                let mut c = tokio::process::Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            }
        };
        // Everything launched from a suite is, by construction, INSIDE a prova run — so stamp the
        // nesting depth here, at the one place both `shell.run` and `shell.spawn` pass through,
        // rather than trying to recognize which commands are prova. That recognition is not
        // available: a string command is opaque (`sh -c "…"`), and the prova that matters is often
        // reached through a wrapper. Stamping unconditionally also makes the marker transitive —
        // `make` forwards its environment, so the prova three levels down still learns the truth.
        // On anything that is not prova it is an inert variable.
        //
        // Set BEFORE the caller's `env = { … }` overlay is applied, so a suite can override the
        // depth deliberately (proving the top-level behavior from inside a nested run needs exactly
        // that) while the default costs the author nothing.
        c.env(crate::RUN_DEPTH_ENV, (crate::run_depth() + 1).to_string());
        c
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
