//! The `shell` module (`shell.run` / `shell.spawn` — async child processes) and the
//! `prova.containerized` scaffolding recipe it loads alongside.

use std::sync::Arc;
use std::time::Instant;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

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

    -- `files` carries configuration IN rather than baking it into an image
    -- (agent-ergonomics.md#containerized-mounts). A caller's entries win over the recipe's by
    -- path, so a topology can override one config file without forking the recipe.
    local files = spec.files
    if type(files) == "function" then files = files(opts) end
    if opts.files then
      local merged = {}
      for k, v in pairs(files or {}) do merged[k] = v end
      for k, v in pairs(opts.files) do merged[k] = v end
      files = merged
    end

    local container = ctx:manage(docker.run{
      image = image, ports = ports, env = env, command = spec.command, wait = wait,
      network = network, alias = alias, extra_hosts = extra_hosts, files = files,
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
    // Held while the process runs; dropped when it is stopped or reaped, so prova's own death
    // sweeps a still-live spawn (docs/design/verifiers.md#conduct-lease-survives-prova-death).
    lease: Option<crate::lease::Lease>,
}

/// Cap for a spawned process's captured output. Old bytes drop first.
pub(super) const SPAWN_OUTPUT_CAP: usize = 64 * 1024;

pub(super) fn spawn_output_reader(
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
                // On unix the spawn is its own process GROUP: stop the whole tree, exactly as
                // every bounded conduct's kill does — a booted app's workers must not outlive
                // the app (docs/design/verifiers.md#conduct-process-group-reaping).
                crate::lease::kill_group(this.pid);
                let _ = child.kill().await;
                this.lease.take();
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
                    this.lease.take(); // exited on its own — nothing left to sweep
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
                this.lease.take(); // reaped — nothing left to sweep
            }
            Ok(running)
        });
    }
}

/// The options `shell.run` takes, extracted up front (owned) so nothing borrows Lua across the
/// await in the conduct.
struct RunOpts {
    cwd: Option<String>,
    env: Vec<(String, String)>,
    timeout: Option<std::time::Duration>,
    /// Liveness bound (docs/design/verifiers.md#conduct-heartbeat-not-deadline): kill the command
    /// only when NO bytes arrive on either stream for this long. Bounds silence, never work —
    /// the wall-clock `timeout` stays the optional outer bound; the two compose.
    idle_timeout: Option<std::time::Duration>,
    /// Start-up bound (docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently): kill
    /// the command if NOTHING arrives on either stream within this long of spawn. The one interval
    /// a caller can bound tightly without knowing the work — a tool that has not spoken at all has
    /// not started — and the first byte disarms it for good.
    first_byte: Option<std::time::Duration>,
    check: bool,
    merge_stderr: bool,
    stdin: Option<String>,
}

/// Every option `shell.run` honors — closed by construction
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
const RUN_OPTS: &[&str] = &[
    "check",
    "cwd",
    "env",
    "first_byte",
    "idle_timeout",
    "merge_stderr",
    "stdin",
    "timeout",
];

/// Every option `shell.spawn` honors. Deliberately NOT `RUN_OPTS`: a spawned process is a handle
/// the caller supervises, so `timeout`/`check`/`stdin` have nothing to act on here. They were
/// silently dropped before this gate — which is the worst possible answer to
/// `shell.spawn(cmd, { timeout = "30s" })`, since the author is asking for a bound and getting
/// none.
const SPAWN_OPTS: &[&str] = &["cwd", "env"];

/// `args` is the option every other process API in the world takes, and prova takes none — the
/// arguments belong in the command itself, as an argv table. Field evidence, 2026-08-14:
/// `shell.spawn("kubectl", { args = {…} })` started a bare `kubectl` that printed usage into a
/// discarded stdout, and the run failed minutes later waiting for an effect nobody had asked for.
///
/// `spawn` is the worst possible host for a dropped option precisely because the process still
/// STARTS: a no-op option leaves a proof proving nothing, but a dropped `args` leaves a *different
/// command* running, so the failure arrives somewhere else wearing an unrelated diagnosis.
const ARGV_TEACHING: &[crate::opts::Teaching] = &[(
    "args",
    "is not an option — the arguments are part of the command: pass an argv table \
     (`{ \"kubectl\", \"get\", \"pods\" }`), which runs the program directly with no shell and no \
     quoting, or a single string (`\"kubectl get pods\"`), which goes through a shell",
)];

/// Both shell verbs' gate, carrying the `args` teaching — the mistake is identical on either, and
/// on `run` it is just as silent (a bare `kubectl` exits non-zero with usage on stdout, which
/// without `check = true` is a result the proof happily reads).
fn reject_shell_opts(opts: &Option<Table>, accepted: &[&str], site: &str) -> mlua::Result<()> {
    let Some(t) = opts else { return Ok(()) };
    crate::opts::Closed {
        accepted,
        hidden: &[],
        teachings: ARGV_TEACHING,
        example: None,
    }
    .check(t, site)
}

/// One duration option, refused rather than dropped when it cannot be read.
fn opt_duration(
    opts: &Option<Table>,
    key: &str,
    site: &str,
) -> mlua::Result<Option<std::time::Duration>> {
    match opt_string(opts, key)? {
        Some(s) => crate::model::require_duration(site, key, &s)
            .map(Some)
            .map_err(mlua::Error::RuntimeError),
        None => Ok(None),
    }
}

fn parse_run_opts(opts: &Option<Table>) -> mlua::Result<RunOpts> {
    reject_shell_opts(opts, RUN_OPTS, "shell.run")?;
    Ok(RunOpts {
        cwd: opt_string(opts, "cwd")?,
        env: opt_env(opts)?,
        timeout: opt_duration(opts, "timeout", "shell.run")?,
        idle_timeout: opt_duration(opts, "idle_timeout", "shell.run")?,
        // `first_byte = "0s"` disables it explicitly — a zero is a choice, not a parse failure.
        first_byte: opt_duration(opts, "first_byte", "shell.run")?.filter(|d| !d.is_zero()),
        check: opts
            .as_ref()
            .map(|o| o.get::<Option<bool>>("check"))
            .transpose()?
            .flatten()
            .unwrap_or(false),
        // Fold stderr into stdout in the result — the portable replacement for `2>&1`.
        merge_stderr: opts
            .as_ref()
            .map(|o| o.get::<Option<bool>>("merge_stderr"))
            .transpose()?
            .flatten()
            .unwrap_or(false),
        // Feed the program's stdin — the portable replacement for a `printf x | cmd` pipe.
        stdin: opt_string(opts, "stdin")?,
    })
}

/// Cumulative CPU consumed by `pid`, in opaque monotonic ticks — units are irrelevant, only
/// increase matters: the question is "did the child do WORK since the last look", never "how
/// much". Native readers only (procfs, libproc): shelling out to `ps` would trade a guess about
/// output cadence for a guess about ps dialects — the same disease, one tool over. `None` — the
/// process is gone, or a platform with no reader — degrades supervision to bytes-only.
#[cfg(target_os = "linux")]
fn child_cpu_ticks(pid: u32) -> Option<u64> {
    // /proc/<pid>/stat, split from the LAST ')' so a comm with spaces or parens cannot shift
    // the fields: the tokens after it start at state (field 3); utime/stime are fields 14/15.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    let mut fields = rest.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime.saturating_add(stime))
}

#[cfg(target_os = "macos")]
fn child_cpu_ticks(pid: u32) -> Option<u64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: the V2 flavor fills exactly the rusage_info_v2 layout; the double cast is the
    // API's own calling convention (rusage_info_t is a void*).
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
        )
    };
    if rc != 0 {
        return None;
    }
    Some(info.ri_user_time.saturating_add(info.ri_system_time))
}

#[cfg(windows)]
fn child_cpu_ticks(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a query-only handle, closed on every path; GetProcessTimes fills four FILETIMEs.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let ticks =
            |ft: FILETIME| ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        Some(ticks(kernel).saturating_add(ticks(user)))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn child_cpu_ticks(_pid: u32) -> Option<u64> {
    None
}

/// One idle-window verdict: did the child accrue CPU since the last look? Updates the watermark
/// on progress. `false` also covers "no reader on this platform" and "process gone" — both
/// degrade to bytes-only supervision, the stricter posture.
fn cpu_advanced(pid: Option<u32>, last: &mut Option<u64>) -> bool {
    let now = pid.and_then(child_cpu_ticks);
    match (now, *last) {
        (Some(now), Some(prev)) if now > prev => {
            *last = Some(now);
            true
        }
        _ => false,
    }
}

/// Put the child in its own process group (unix), so a kill can be a GROUP kill
/// (docs/design/verifiers.md#conduct-process-group-reaping) and the lease's sweep has one
/// address for the whole tree. Non-unix: no-op (job objects are the windows lane's business).
pub(super) fn isolate_group(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

/// How a supervised conduct ended: ran to completion, or was killed for silence (carrying what
/// had been captured when the idle clock ran out, so the error can show where it stalled).
pub(super) enum Supervised {
    Finished(std::process::Output),
    Idle { stdout: Vec<u8>, stderr: Vec<u8> },
    /// The wall clock fired — the unconditional outer bound, killed as a GROUP like every
    /// controlled kill (docs/design/verifiers.md#conduct-process-group-reaping).
    Wall { stdout: Vec<u8>, stderr: Vec<u8> },
    /// The first-byte clock fired: the tool said NOTHING on either stream, so it never started.
    /// Carries no tails — there are none, and that absence is the whole finding.
    Mute,
}

/// Drive a child under supervision — every bounded conduct routes here, so every bound's kill
/// is explicit code that can reap the whole GROUP (never a dropped future's child-only kill).
/// The idle clock (docs/design/verifiers.md#conduct-heartbeat-not-deadline) re-arms on any byte
/// and kills only when a window passes with no bytes AND no CPU progress; the wall clock is the
/// unconditional outer bound. Either may be absent. The exit wait after both streams close is
/// bounded by the same clocks — a child that closed its pipes and lingers doing nothing is
/// dead. `kill_on_drop` stays as the last-resort child-only kill if this future is cancelled
/// some other way; the lease covers prova's own death.
pub(super) async fn run_supervised(
    mut command: tokio::process::Command,
    input: Option<String>,
    idle: Option<std::time::Duration>,
    wall: Option<std::time::Duration>,
    first_byte: Option<std::time::Duration>,
) -> std::io::Result<Supervised> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(match &input {
            Some(_) => std::process::Stdio::piped(),
            None => std::process::Stdio::null(),
        })
        .kill_on_drop(true);
    isolate_group(&mut command);
    let mut child = command.spawn()?;
    let _lease = crate::lease::Lease::register(child.id());
    if let Some(input) = input {
        if let Some(mut si) = child.stdin.take() {
            si.write_all(input.as_bytes()).await?;
            si.shutdown().await?; // close so the child sees EOF
        }
    }
    let mut out = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout was not captured"))?;
    let mut err = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("child stderr was not captured"))?;
    let pid = child.id();
    let mut last_cpu = pid.and_then(child_cpu_ticks);
    let deadline = wall.map(|w| tokio::time::Instant::now() + w);
    // Armed at spawn, disarmed by the first byte on either stream — a tool answers once, and after
    // that the question ("did it ever start?") is settled for the rest of the conduct.
    let mut mute_until = first_byte.map(|f| tokio::time::Instant::now() + f);
    // With no idle bound, the idle arm still ticks (as a coarse heartbeat interval) but never
    // kills — the check below requires an actual idle bound to act.
    let tick = idle.unwrap_or(std::time::Duration::from_secs(3600));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);
    let (mut buf_o, mut buf_e) = ([0u8; 8192], [0u8; 8192]);
    while !(out_done && err_done) {
        tokio::select! {
            r = out.read(&mut buf_o), if !out_done => match r? {
                0 => out_done = true,
                n => {
                    mute_until = None;
                    stdout.extend_from_slice(&buf_o[..n]);
                }
            },
            r = err.read(&mut buf_e), if !err_done => match r? {
                0 => err_done = true,
                n => {
                    mute_until = None;
                    stderr.extend_from_slice(&buf_e[..n]);
                }
            },
            // Recreated each iteration, so every chunk read above re-arms the clock. Silence on
            // the pipes is only HALF the evidence when it fires: a big compile says nothing for
            // minutes while working flat-out, so a quiet window with CPU progress is life —
            // kill only when both are absent (bytes OR work is the heartbeat).
            () = tokio::time::sleep(tick) => {
                if idle.is_some() && !cpu_advanced(pid, &mut last_cpu) {
                    crate::lease::kill_group(pid);
                    child.kill().await.ok();
                    return Ok(Supervised::Idle { stdout, stderr });
                }
            }
            // The wall clock: fires regardless of how healthy the heartbeat is — the two bounds
            // answer different questions (is it dead? / may it keep going?).
            () = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                crate::lease::kill_group(pid);
                child.kill().await.ok();
                return Ok(Supervised::Wall { stdout, stderr });
            }
            // The first-byte clock: a third question — did it ever START? Unlike the idle clock it
            // ignores CPU entirely, because a tool wedged on a hung daemon burns none and one that
            // is merely slow to speak has still said nothing a caller can act on.
            () = tokio::time::sleep_until(mute_until.unwrap_or_else(tokio::time::Instant::now)), if mute_until.is_some() => {
                crate::lease::kill_group(pid);
                child.kill().await.ok();
                return Ok(Supervised::Mute);
            }
        }
    }
    // Streams closed, exit pending: the same clocks and the same evidence bound the wait — a
    // child that closed its pipes but still computes is alive; one that lingers doing nothing
    // is dead.
    loop {
        let wait_slice = deadline
            .map(|d| d.saturating_duration_since(tokio::time::Instant::now()).min(tick))
            .unwrap_or(tick);
        match tokio::time::timeout(wait_slice, child.wait()).await {
            Ok(status) => {
                return Ok(Supervised::Finished(std::process::Output {
                    status: status?,
                    stdout,
                    stderr,
                }))
            }
            Err(_) => {
                let wall_fired = deadline.is_some_and(|d| tokio::time::Instant::now() >= d);
                if wall_fired {
                    crate::lease::kill_group(pid);
                    child.kill().await.ok();
                    return Ok(Supervised::Wall { stdout, stderr });
                }
                if idle.is_some() && !cpu_advanced(pid, &mut last_cpu) {
                    crate::lease::kill_group(pid);
                    child.kill().await.ok();
                    return Ok(Supervised::Idle { stdout, stderr });
                }
            }
        }
    }
}

/// One `shell.run` conduct: build the command, bracket the blocking region with progress, drive
/// it under the stdin/timeout policy, and fold the streams per `merge_stderr`.
async fn run_command(
    cmd: &CommandSpec,
    o: &RunOpts,
    progress: &Arc<dyn Progress>,
) -> mlua::Result<ShellResult> {
    let mut command = cmd.build();
    if let Some(dir) = &o.cwd {
        command.current_dir(dir);
    }
    for (k, v) in &o.env {
        command.env(k, v);
    }

    // Bracket the blocking region: a captured build says nothing until it exits, which is
    // pause #2 in the inventory. The renderer decides whether this is worth a line — a 30ms
    // `echo` stays silent, a two-minute `cargo build` does not.
    let activity = progress::start(progress, Kind::Command, cmd.display_name());
    let start = Instant::now();
    // With `stdin`, pipe the input in and reap via wait_with_output; otherwise `output()`
    // with stdin EXPLICITLY nulled. tokio's `output()` — unlike std's — only forces
    // stdout/stderr and lets stdin INHERIT, so a child that reads stdin (a journaling
    // shim's `cat`, an interactive prompt) blocks forever whenever the harness's own
    // stdin is a non-closing pipe. That was a live 40-minute suite hang under the
    // coverage conduct. Hermetic default: a test's child sees EOF, never the harness's
    // stdin; a proof that means to feed input says `stdin = ...`.
    // Dead means dead (docs/design/verifiers.md#timeout-reaps-the-conduct): every BOUNDED run
    // routes through the supervisor, so every bound's kill is explicit code that reaps the whole
    // group (docs/design/verifiers.md#conduct-process-group-reaping). kill_on_drop stays as the
    // last-resort child-only kill for an externally-cancelled future.
    command.kill_on_drop(true);
    let output = if o.idle_timeout.is_some() || o.timeout.is_some() || o.first_byte.is_some() {
        let supervised =
            run_supervised(command, o.stdin.clone(), o.idle_timeout, o.timeout, o.first_byte)
                .await
            .map_err(|e| mlua::Error::RuntimeError(format!("shell.run failed to spawn: {e}")))?;
        match supervised {
            Supervised::Finished(output) => output,
            Supervised::Idle { stdout, stderr } => {
                let idle = o.idle_timeout.unwrap_or_default();
                return Err(mlua::Error::RuntimeError(format!(
                    "shell.run: no output and no CPU progress for {idle:?} (idle_timeout) — \
                     killed as dead: {cmd}\n\
                     --- stderr tail ---\n{}\n--- stdout tail ---\n{}",
                    tail(&String::from_utf8_lossy(&stderr), 4096),
                    tail(&String::from_utf8_lossy(&stdout), 4096),
                )));
            }
            Supervised::Wall { stdout, stderr } => {
                let budget = o.timeout.unwrap_or_default();
                return Err(mlua::Error::RuntimeError(format!(
                    "shell.run timed out after {budget:?}: {cmd}\n\
                     --- stderr tail ---\n{}\n--- stdout tail ---\n{}",
                    tail(&String::from_utf8_lossy(&stderr), 4096),
                    tail(&String::from_utf8_lossy(&stdout), 4096),
                )));
            }
            // No tails to show, and their absence IS the report: this command never spoke, so it
            // never started. Said as a different sentence from a timeout on purpose — "slow" and
            // "never answered" send an operator to different places.
            Supervised::Mute => {
                let bound = o.first_byte.unwrap_or_default();
                return Err(mlua::Error::RuntimeError(format!(
                    "shell.run: no output at all within {bound:?} (first_byte) — the command never \
                     answered, so it never started: {cmd}"
                )));
            }
        }
    } else {
        // Unbounded: the buffered wait, isolated and leased like every conduct, so prova's own
        // death still sweeps the group even though no bound will ever fire here.
        isolate_group(&mut command);
        let run = async {
            if let Some(input) = &o.stdin {
                use tokio::io::AsyncWriteExt;
                command
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let mut child = command.spawn()?;
                let _lease = crate::lease::Lease::register(child.id());
                if let Some(mut si) = child.stdin.take() {
                    si.write_all(input.as_bytes()).await?;
                    si.shutdown().await?; // close so the child sees EOF
                }
                child.wait_with_output().await
            } else {
                command.stdin(std::process::Stdio::null());
                command
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let child = command.spawn()?;
                let _lease = crate::lease::Lease::register(child.id());
                child.wait_with_output().await
            }
        };
        run.await
            .map_err(|e| mlua::Error::RuntimeError(format!("shell.run failed to spawn: {e}")))?
    };

    let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if o.merge_stderr {
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
    Ok(result)
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
                    super::runtime_only("shell.run")?;
                    // A string runs through a shell (`"cargo build --release"` verbatim); an argv
                    // table runs the program directly — no shell, no quoting.
                    let cmd = CommandSpec::parse(cmd)?;
                    let o = parse_run_opts(&opts)?;
                    let result = run_command(&cmd, &o, &progress).await?;
                    if o.check && result.code != 0 {
                        // Builds put failure detail on either stream (msbuild/pnpm favor stdout),
                        // so the error carries the tail of both — better than a hand-rolled assert.
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
            super::runtime_only("shell.spawn")?;
            let cmd = CommandSpec::parse(cmd)?;
            reject_shell_opts(&opts, SPAWN_OPTS, "shell.spawn")?;
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
            isolate_group(&mut command);
            let mut child = command
                .spawn()
                .map_err(|e| mlua::Error::RuntimeError(format!("shell.spawn failed: {e}")))?;
            let pid = child.id();
            let lease = Some(crate::lease::Lease::register(pid));
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
                lease,
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
pub(super) enum CommandSpec {
    Shell(String),
    Argv(Vec<String>),
}

impl CommandSpec {
    /// A short label for an activity line. Truncated hard: a `cargo build` invocation with twenty
    /// flags is not what someone staring at a stalled run needs — the program and a hint of its
    /// arguments is. The full command is still in the error on failure.
    pub(super) fn display_name(&self) -> String {
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

    pub(super) fn parse(v: mlua::Value) -> mlua::Result<Self> {
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

    pub(super) fn build(&self) -> tokio::process::Command {
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
pub(super) fn env_value(key: &str, v: Value) -> mlua::Result<String> {
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
