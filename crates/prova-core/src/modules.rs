//! First-party capability modules injected as globals alongside `prova`: `shell`, `fs`, `http`,
//! `docker`, and `db`.
//!
//! These are what make prova useful beyond testing itself — bring a system into existence and poke
//! it. `shell.run`/`shell.spawn`, `http.*`, and `docker.*` are async (child processes / requests /
//! docker calls never block the worker); `fs` is synchronous (fast metadata/read ops). All take
//! context explicitly (no ambient cwd), preserving the isolation the design promises. `http` is
//! behind a default-on feature and is HTTP-only in v1 (an `https`/TLS feature can layer on later);
//! `docker` uses the typed **bollard** daemon client, so tests that need it declare
//! `requires = { "docker" }` to skip gracefully where the daemon is absent. `http`/`db`/`docker` are
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
    #[cfg(feature = "docker")]
    lua.globals().set("docker", docker::make(lua)?)?;
    #[cfg(feature = "http")]
    lua.globals().set("http", http::make(lua)?)?;
    #[cfg(feature = "db")]
    lua.globals().set("db", db::make(lua)?)?;
    #[cfg(feature = "grpc")]
    lua.globals().set("grpc", grpc::make(lua)?)?;
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
    use bollard::exec::{CreateExecOptions, StartExecResults};
    use bollard::image::CreateImageOptions;
    use bollard::models::{HostConfig, PortBinding};
    use bollard::Docker;
    use futures::StreamExt;
    use mlua::{Function, Lua, Table, UserData, UserDataFields, UserDataMethods};

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
            methods.add_async_method("exec", |_, this, cmd: String| {
                let client = this.client.clone();
                let id = this.id.clone();
                async move { container_exec(&client, &id, cmd).await }
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
        for port in &spec.ports {
            let key = format!("{port}/tcp");
            exposed.insert(key.clone(), HashMap::new());
            bindings.insert(
                key,
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".to_string()),
                    host_port: Some("0".to_string()),
                }]),
            );
        }

        let config = Config {
            image: Some(spec.image.clone()),
            env: Some(spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
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
            for port in &spec.ports {
                if let Some(Some(binds)) = ports.get(&format!("{port}/tcp")) {
                    if let Some(hp) = binds
                        .first()
                        .and_then(|b| b.host_port.as_ref())
                        .and_then(|s| s.parse::<u16>().ok())
                    {
                        container.ports.insert(*port, hp);
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

    async fn container_exec(
        client: &Docker,
        id: &str,
        cmd: String,
    ) -> mlua::Result<(i64, String, String)> {
        let exec = client
            .create_exec(
                id,
                CreateExecOptions {
                    cmd: Some(vec!["sh".to_string(), "-c".to_string(), cmd]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(derr)?;
        let (mut stdout, mut stderr) = (String::new(), String::new());
        if let StartExecResults::Attached { mut output, .. } =
            client.start_exec(&exec.id, None).await.map_err(derr)?
        {
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

// ---------------------------------------------------------------------------------------------
// grpc (async; native — no `grpcurl` binary. Plaintext-only in v1, like http.)
// ---------------------------------------------------------------------------------------------

// A *dynamic* gRPC client: it learns the server's schema at runtime via gRPC Server Reflection
// (so tests need no `.proto` files and no codegen), builds request messages from Lua tables against
// the fetched descriptors, invokes with a generic tonic codec over `DynamicMessage`, and decodes the
// reply back to a Lua table. This keeps prova a single self-contained binary — the whole point of
// going native instead of shelling out to `grpcurl`. The server must have reflection enabled; if it
// doesn't, `grpc.connect` fails with a clear message (a proto-file path mode can layer on later).
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
        // grpc.connect(addr, { timeout = "30s" }) → a Client (reflection is performed here, once).
        grpc.set(
            "connect",
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
    /// fields present so assertions can see the full message shape.
    fn response_to_lua(lua: &Lua, msg: &DynamicMessage) -> mlua::Result<Value> {
        let opts = SerializeOptions::new().skip_default_fields(false);
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
