//! First-party capability modules injected as globals alongside `prova`: `shell`, `fs`, `http`,
//! `docker`, and `db`.
//!
//! These are what make prova useful beyond testing itself — bring a system into existence and poke
//! it. `shell.run`/`shell.spawn`, `http.*`, and `docker.*` are async (child processes / requests /
//! docker calls never block the worker); `fs` is synchronous (fast metadata/read ops). All take
//! context explicitly (no ambient cwd), preserving the isolation the design promises. `http` is
//! behind a default-on feature and is HTTP-only in v1 (an `https`/TLS feature can layer on later);
//! `docker` shells out to the `docker` CLI, so tests that need it declare `requires = { "docker" }`
//! to skip gracefully where the daemon is absent.

use std::path::Path;
use std::time::Instant;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods};

use crate::model::parse_duration;

/// Install the built-in module globals (`shell`, `fs`, `docker`, and — with the `http` feature —
/// `http`) into `lua`.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set("shell", make_shell(lua)?)?;
    lua.globals().set("fs", make_fs(lua)?)?;
    lua.globals().set("docker", docker::make(lua)?)?;
    #[cfg(feature = "http")]
    lua.globals().set("http", http::make(lua)?)?;
    #[cfg(feature = "db")]
    lua.globals().set("db", db::make(lua)?)?;
    Ok(())
}

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
            // Decode the body as JSON into a Lua value; raises on non-JSON.
            methods.add_method("json", |lua, this, ()| {
                let value: serde_json::Value = serde_json::from_str(&this.body).map_err(|e| {
                    mlua::Error::RuntimeError(format!("response body is not JSON: {e}"))
                })?;
                lua.to_value(&value)
            });
        }
    }

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let http = lua.create_table()?;
        http.set("get", method_fn(lua, reqwest::Method::GET)?)?;
        http.set("post", method_fn(lua, reqwest::Method::POST)?)?;
        http.set("put", method_fn(lua, reqwest::Method::PUT)?)?;
        http.set("delete", method_fn(lua, reqwest::Method::DELETE)?)?;
        http.set("wait_for", wait_for_fn(lua)?)?;
        Ok(http)
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
            let prepared = prepare(&lua, method.clone(), url, opts);
            async move {
                let resp = send(prepared?).await?;
                lua.create_userdata(resp)
            }
        })
    }

    fn prepare(
        lua: &Lua,
        method: reqwest::Method,
        url: String,
        opts: Option<Table>,
    ) -> mlua::Result<Prepared> {
        let mut headers = Vec::new();
        let mut body = None;
        let mut timeout = None;
        if let Some(opts) = opts {
            if let Some(hdrs) = opts.get::<Option<Table>>("headers")? {
                for pair in hdrs.pairs::<String, String>() {
                    let (k, v) = pair?;
                    headers.push((k, v));
                }
            }
            if let Some(json) = opts.get::<Option<Value>>("json")? {
                let value: serde_json::Value = lua.from_value(json)?;
                let encoded = serde_json::to_vec(&value).map_err(|e| {
                    mlua::Error::RuntimeError(format!("http: encoding json body: {e}"))
                })?;
                headers.push(("content-type".into(), "application/json".into()));
                body = Some(encoded);
            } else if let Some(raw) = opts.get::<Option<String>>("body")? {
                body = Some(raw.into_bytes());
            }
            timeout = opts
                .get::<Option<String>>("timeout")?
                .and_then(|s| parse_duration(&s));
        }
        Ok(Prepared {
            method,
            url,
            headers,
            body,
            timeout,
        })
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
// docker (testcontainers-style ephemeral dependencies, via the `docker` CLI)
// ---------------------------------------------------------------------------------------------

mod docker {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods};

    use crate::model::parse_duration;

    /// A running container from `docker.run` — the testcontainers primitive. `c.id`,
    /// `c:host_port(p)`, `c:endpoint(p)`, async `c:logs()`, `c:exec(cmd)`, `c:stop()`. Started with
    /// `--rm`; `stop` force-removes it, and a `Drop` backstop removes it if a test forgot to. The
    /// blessed pattern is `ctx:defer(function() c:stop() end)` so it is torn down (async teardown)
    /// with the container still owned by the run.
    struct Container {
        id: String,
        ports: HashMap<u16, u16>, // container port -> mapped host port
        stopped: bool,
    }

    impl Drop for Container {
        fn drop(&mut self) {
            if !self.stopped {
                // Best-effort, fire-and-forget: never leak a container even if cleanup was skipped.
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
            // The host port a published container port maps to.
            methods.add_method("host_port", |_, this, port: u16| {
                this.ports.get(&port).copied().ok_or_else(|| {
                    mlua::Error::RuntimeError(format!("container port {port} was not published"))
                })
            });
            // Convenience: "127.0.0.1:<host_port>" for the published port.
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
                let id = this.id.clone();
                async move {
                    let (_, out, err) = output(&["logs".into(), id]).await?;
                    Ok(format!("{out}{err}"))
                }
            });
            methods.add_async_method("exec", |_, this, cmd: String| {
                let id = this.id.clone();
                async move {
                    let (code, stdout, stderr) =
                        output(&["exec".into(), id, "sh".into(), "-c".into(), cmd]).await?;
                    Ok((code, stdout, stderr))
                }
            });
            // Force-remove the container. Idempotent.
            methods.add_async_method_mut("stop", |_, mut this, ()| async move {
                if !this.stopped {
                    this.stopped = true;
                    let _ = output(&["rm".into(), "-f".into(), this.id.clone()]).await;
                }
                Ok(())
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
            // Extract everything synchronously (owned) up front — nothing borrows Lua across awaits.
            let spec = Spec::from_table(&opts);
            async move {
                let spec = spec?;
                let container = start(spec).await?;
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
        ports: Vec<u16>,
        env: Vec<(String, String)>,
        wait: Option<Wait>,
    }

    impl Spec {
        fn from_table(opts: &Table) -> mlua::Result<Spec> {
            let image = opts.get::<Option<String>>("image")?.ok_or_else(|| {
                mlua::Error::RuntimeError("docker.run requires an `image`".into())
            })?;
            let ports = opts.get::<Option<Vec<u16>>>("ports")?.unwrap_or_default();
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
                wait,
            })
        }
    }

    async fn start(spec: Spec) -> mlua::Result<Container> {
        let mut args = vec!["run".into(), "-d".into(), "--rm".into()];
        for port in &spec.ports {
            args.push("-p".into());
            args.push(format!("0:{port}")); // publish to a random host port
        }
        for (k, v) in &spec.env {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        args.push(spec.image.clone());

        let (code, stdout, stderr) = output(&args).await?;
        if code != 0 {
            return Err(mlua::Error::RuntimeError(format!(
                "docker run failed for {:?}: {}",
                spec.image,
                stderr.trim()
            )));
        }
        let id = stdout.trim().to_string();

        // A container that fails partway is force-removed via Container::drop.
        let mut container = Container {
            id: id.clone(),
            ports: HashMap::new(),
            stopped: false,
        };
        for port in &spec.ports {
            let (c, out, _) = output(&["port".into(), id.clone(), format!("{port}/tcp")]).await?;
            if c == 0 {
                if let Some(host_port) = parse_host_port(&out) {
                    container.ports.insert(*port, host_port);
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
                let (_, out, err) = output(&["logs".into(), container.id.clone()]).await?;
                out.contains(pattern.as_str()) || err.contains(pattern.as_str())
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

    /// Run `docker <args>`, returning (exit code, stdout, stderr).
    async fn output(args: &[String]) -> mlua::Result<(i32, String, String)> {
        let out = tokio::process::Command::new("docker")
            .args(args)
            .output()
            .await
            .map_err(|e| mlua::Error::RuntimeError(format!("failed to run docker: {e}")))?;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// Parse the host port from `docker port` output ("0.0.0.0:49154\n[::]:49154").
    fn parse_host_port(out: &str) -> Option<u16> {
        out.lines().next()?.rsplit(':').next()?.trim().parse().ok()
    }
}

// ---------------------------------------------------------------------------------------------
// db (one general query API over Postgres/MySQL/SQLite via sqlx's `Any` driver)
// ---------------------------------------------------------------------------------------------

#[cfg(feature = "db")]
mod db {
    use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
    use sqlx::any::{AnyPoolOptions, AnyRow, AnyTypeInfoKind};
    use sqlx::{AnyPool, Column, Row};

    /// A database connection pool from `db.connect(url)`. The backend is chosen by URL scheme
    /// (`postgres://`, `mysql://`, `sqlite://…?mode=rwc`), so one API covers all three. Methods are
    /// async; pair with `ctx:defer(function() conn:close() end)`.
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

    pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
        let db = lua.create_table()?;
        db.set("connect", connect_fn(lua)?)?;
        Ok(db)
    }

    fn connect_fn(lua: &Lua) -> mlua::Result<Function> {
        lua.create_async_function(|lua, url: String| async move {
            sqlx::any::install_default_drivers(); // idempotent
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .map_err(|e| mlua::Error::RuntimeError(format!("db.connect {url:?}: {e}")))?;
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
                    "db: unsupported parameter type {}",
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
        mlua::Error::RuntimeError(format!("db error: {e}"))
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
