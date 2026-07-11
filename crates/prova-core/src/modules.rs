//! First-party capability modules injected as globals alongside `prova`: `shell`, `fs`, and `http`.
//!
//! These are what make prova useful beyond testing itself — bring a system into existence and poke
//! it. `shell.run` and `http.*` are async (child processes / requests never block the worker);
//! `fs` is synchronous (fast metadata/read ops). All take context explicitly (no ambient cwd),
//! preserving the isolation the design promises. `http` is behind a default-on feature and is
//! HTTP-only in v1 (an `https`/TLS feature can layer on later).

use std::path::Path;
use std::time::Instant;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods};

use crate::model::parse_duration;

/// Install the `shell`, `fs` (and, with the `http` feature, `http`) module globals into `lua`.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set("shell", make_shell(lua)?)?;
    lua.globals().set("fs", make_fs(lua)?)?;
    #[cfg(feature = "http")]
    lua.globals().set("http", http::make(lua)?)?;
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
