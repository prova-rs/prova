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

pub(crate) fn make(
    lua: &Lua,
    progress: &super::Arc<dyn super::Progress>,
) -> mlua::Result<Table> {
    let docker = lua.create_table()?;
    docker.set("run", run_fn(lua, progress)?)?;
    docker.set("build", build_fn(lua, progress)?)?;
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

fn run_fn(
    lua: &Lua,
    progress: &super::Arc<dyn super::Progress>,
) -> mlua::Result<Function> {
    let progress = super::Arc::clone(progress);
    lua.create_async_function(move |lua, opts: Table| {
        let progress = super::Arc::clone(&progress);
        let spec = Spec::from_table(&opts);
        async move {
            super::runtime_only("docker.run")?;
            let container = start(spec?, &progress).await?;
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
    secrets: Vec<(String, BuildSecret)>,
    target: Option<String>,
    pull: bool,
    nocache: bool,
    /// How long the builder may say NOTHING before it is declared wedged
    /// (docs/design/agent-ergonomics.md#buildkit-wedge-hangs-suites-silently). A healthy BuildKit
    /// prints `load build definition` in about a second; a wedged one prints nothing, ever. This
    /// bounds only the silence BEFORE the first byte, so an hour-long build is never touched.
    first_byte: Option<std::time::Duration>,
}

/// How long `docker.build` waits for the builder's first byte before calling it wedged. Generous
/// against a cold daemon (Docker Desktop can take tens of seconds to answer its socket after a
/// laptop wakes) and still ~80× tighter than the suite bound the wedge used to consume.
const DEFAULT_BUILD_FIRST_BYTE: std::time::Duration = std::time::Duration::from_secs(90);

/// Where a BuildKit secret's bytes come from. A production Dockerfile that reads a private
/// registry token via `RUN --mount=type=secret,id=…` cannot be built without this, and a
/// build arg is not a substitute: build args are baked into image history, which is exactly
/// what BuildKit secrets exist to avoid.
enum BuildSecret {
    /// `--secret id=…,env=VAR` — the daemon reads the named variable from our environment.
    Env(String),
    /// `--secret id=…,src=PATH` — a file already on disk.
    File(String),
    /// A literal value from Lua. Written to a private temp file for the duration of the build
    /// and removed after, because the Docker CLI has no way to take a secret on stdin.
    Value(String),
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

/// Every option `docker.build` honors — closed by construction
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored). This is the surface the
/// backlog item was found on: `docker.build{ first_byte = … }` against a prova built without the
/// option parsed clean and proved nothing about the bound it named.
const BUILD_OPTS: &[&str] = &[
    "buildargs",
    "context",
    "dockerfile",
    "first_byte",
    "nocache",
    "pull",
    "secrets",
    "tag",
    "target",
];

/// Every option `docker.run` honors.
const RUN_OPTS: &[&str] = &[
    "alias",
    "command",
    "env",
    "extra_hosts",
    "files",
    "image",
    "network",
    "ports",
    "wait",
];

/// Every key `docker.run`'s nested `wait` table honors. Gated separately because a typo *inside*
/// `wait` is the more dangerous of the two: `wait = { prot = 5432 }` is a readiness contract that
/// waits for nothing, so the container is handed over unready and the failure lands somewhere else
/// entirely.
const WAIT_OPTS: &[&str] = &["cmd", "every", "log", "port", "timeout"];

impl BuildSpec {
    fn from_table(t: &Table) -> mlua::Result<Self> {
        crate::opts::reject_unknown(t, BUILD_OPTS, "docker.build")?;
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

        // `secrets = { ["id"] = { env = "VAR" } | { file = "path" } | { value = "…" } }`.
        // Deliberately no bare-string shorthand: a string would be ambiguous between a path and
        // a literal secret, and guessing wrong either leaks the value into the build or silently
        // mounts the wrong bytes.
        let mut secrets = Vec::new();
        if let Some(tbl) = t.get::<Option<Table>>("secrets")? {
            for pair in tbl.pairs::<String, Value>() {
                let (id, v) = pair?;
                let src = match v {
                    Value::Table(spec) => {
                        let env = spec.get::<Option<String>>("env")?;
                        let file = spec.get::<Option<String>>("file")?;
                        let value = spec.get::<Option<String>>("value")?;
                        match (env, file, value) {
                            (Some(e), None, None) => BuildSecret::Env(e),
                            (None, Some(f), None) => {
                                if !std::path::Path::new(&f).is_file() {
                                    return Err(mlua::Error::RuntimeError(format!(
                                        "docker.build: secret `{id}` file `{f}` does not exist"
                                    )));
                                }
                                BuildSecret::File(f)
                            }
                            (None, None, Some(val)) => BuildSecret::Value(val),
                            _ => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "docker.build: secret `{id}` needs exactly one of \
                                     `env`, `file`, or `value`"
                                )))
                            }
                        }
                    }
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "docker.build: secret `{id}` must be a table with one of `env`, \
                             `file`, or `value`, got {}",
                            other.type_name()
                        )))
                    }
                };
                secrets.push((id, src));
            }
        }

        Ok(BuildSpec {
            context,
            dockerfile,
            tag,
            buildargs,
            secrets,
            target: t.get::<Option<String>>("target")?,
            pull: t.get::<Option<bool>>("pull")?.unwrap_or(false),
            nocache: t.get::<Option<bool>>("nocache")?.unwrap_or(false),
            // Defaulted, not opt-in: the failure this bound answers cost 2h per suite precisely
            // because nobody had thought to ask for it. `first_byte = "0s"` disables it.
            first_byte: match t.get::<Option<String>>("first_byte")? {
                Some(s) => parse_duration(&s).filter(|d| !d.is_zero()),
                None => Some(DEFAULT_BUILD_FIRST_BYTE),
            },
        })
    }
}

/// `docker.build{ context, dockerfile?, tag?, buildargs?, secrets?, target?, pull?, nocache? }`
/// — build a local image from a Dockerfile and return its ref, ready for
/// `docker.run{ image = … }`.
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
fn build_fn(
    lua: &Lua,
    progress: &super::Arc<dyn super::Progress>,
) -> mlua::Result<Function> {
    let progress = super::Arc::clone(progress);
    lua.create_async_function(move |_, opts: Table| {
        let progress = super::Arc::clone(&progress);
        let spec = BuildSpec::from_table(&opts);
        async move {
            super::runtime_only("docker.build")?;
            let spec = spec?;
            let activity =
                super::progress::start(&progress, super::Kind::Build, spec.tag.clone());
            let out = build(spec).await;
            match &out {
                Ok(_) => activity.done(),
                Err(_) => activity.done_with("failed"),
            }
            out
        }
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

    // Inline `value` secrets need to exist as files for the CLI to read. Hold the temp dir in a
    // guard so it is removed when this function returns — success, failure, or early `?`.
    let secret_dir = if spec
        .secrets
        .iter()
        .any(|(_, s)| matches!(s, BuildSecret::Value(_)))
    {
        Some(SecretDir(
            crate::engine::make_tempdir().map_err(|e| derr(format!("docker.build: {e}")))?,
        ))
    } else {
        None
    };
    for (id, src) in &spec.secrets {
        match src {
            BuildSecret::Env(var) => {
                cmd.arg("--secret").arg(format!("id={id},env={var}"));
            }
            BuildSecret::File(path) => {
                cmd.arg("--secret").arg(format!("id={id},src={path}"));
            }
            BuildSecret::Value(value) => {
                // The guard is created above iff a Value secret exists.
                let dir = &secret_dir
                    .as_ref()
                    .ok_or_else(|| derr("docker.build: secret staging dir was not created"))?
                    .0;
                let path = dir.join(id);
                write_private(&path, value)
                    .map_err(|e| derr(format!("docker.build: secret `{id}`: {e}")))?;
                cmd.arg("--secret")
                    .arg(format!("id={id},src={}", path.display()));
            }
        }
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

    // Supervised like every other conduct (shell.rs owns the clocks): the first-byte bound turns a
    // wedged builder into a named failure in seconds, and routing through the supervisor also puts
    // the build in its own process group under prova's lease — so `kill -9 prova` no longer leaves
    // a `docker build` running.
    let output = match super::shell::run_supervised(cmd, None, None, None, spec.first_byte)
        .await
        .map_err(derr)?
    {
        super::shell::Supervised::Finished(output) => output,
        super::shell::Supervised::Mute => {
            let bound = spec.first_byte.unwrap_or(DEFAULT_BUILD_FIRST_BYTE);
            return Err(derr(format!(
                "docker.build: the builder produced no output at all within {bound:?} — BuildKit \
                 prints `load build definition` within seconds on a healthy builder, so this is a \
                 wedged builder, not a slow build. Restart it (Docker Desktop: restart the app or \
                 `docker buildx rm`), then re-run. Other daemon ops (`docker pull`, `docker ps`) \
                 can stay healthy while buildkitd is wedged, so a green capability probe does not \
                 clear it. Pass `first_byte = \"0s\"` to build unbounded."
            )));
        }
        // No idle or wall bound is set here, so the supervisor cannot answer with either.
        super::shell::Supervised::Idle { .. } | super::shell::Supervised::Wall { .. } => {
            return Err(derr("docker.build: unreachable supervision verdict"))
        }
    };
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

/// Owns the temp dir holding inline secret values, and removes it on drop — so the bytes are
/// gone whether the build succeeded, failed, or panicked.
struct SecretDir(std::path::PathBuf);

impl Drop for SecretDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a secret to `path` readable only by this user. The mode is set *before* the bytes land
/// on unix (create with 0600 rather than writing then chmod'ing), so there is no window where
/// the value is world-readable.
fn write_private(path: &std::path::Path, value: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(value.as_bytes())?;
    f.flush()
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
            super::runtime_only("docker.network")?;
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
    /// A readiness COMMAND run inside the container: ready ⇔ it exits 0. The general,
    /// service-agnostic signal, for the servers where `port` (LISTEN state) is not the same as
    /// application-readiness — Postgres binds its TCP socket and *then* finishes startup,
    /// rejecting queries with "the database system is starting up" in the gap, so a port probe
    /// races. `pg_isready`, `redis-cli ping`, `curl -f /health` — each image's own honest check.
    cmd: Option<Vec<String>>,
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
    /// Content carried INTO the container between create and start
    /// (docs/design/agent-ergonomics.md#containerized-mounts) — never a bind, so it works against
    /// a daemon that does not share this filesystem.
    files: Vec<files::FileEntry>,
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
    /// `ports` entries are either an integer container port (→ random host port) or a table
    /// `{ container = N, host = M }` (→ fixed host port M, needed by e.g. Kafka's advertised
    /// listener). A bare `{ N, M }` array works too.
    fn parse_ports(opts: &Table) -> mlua::Result<Vec<(u16, Option<u16>)>> {
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
        Ok(ports)
    }

    /// The readiness contract. The three signals are honest about *different* observables (a
    /// listening port, a log line, a command's verdict), so combining them would be ambiguous
    /// about what "ready" even means. Exactly one — or none (return-when-started).
    fn parse_wait(opts: &Table) -> mlua::Result<Option<Wait>> {
        let Some(w) = opts.get::<Option<Table>>("wait")? else {
            return Ok(None);
        };
        crate::opts::reject_unknown(&w, WAIT_OPTS, "docker.run `wait`")?;
        let port = w.get::<Option<u16>>("port")?;
        let log = w.get::<Option<String>>("log")?;
        // `cmd` is a command vector (argv), run directly in the container with no shell —
        // same convention as `container:run`'s table form.
        let cmd = w.get::<Option<Vec<String>>>("cmd")?;
        if [port.is_some(), log.is_some(), cmd.is_some()]
            .iter()
            .filter(|set| **set)
            .count()
            > 1
        {
            return Err(mlua::Error::RuntimeError(
                "docker.run `wait` takes exactly one of `port`, `log`, or `cmd` — they \
                 are different readiness signals and cannot be combined"
                    .into(),
            ));
        }
        Ok(Some(Wait {
            port,
            log,
            cmd,
            timeout: w
                .get::<Option<String>>("timeout")?
                .and_then(|s| parse_duration(&s))
                .unwrap_or(Duration::from_secs(30)),
            every: w
                .get::<Option<String>>("every")?
                .and_then(|s| parse_duration(&s))
                .unwrap_or(Duration::from_millis(250)),
        }))
    }

    fn from_table(opts: &Table) -> mlua::Result<Spec> {
        crate::opts::reject_unknown(opts, RUN_OPTS, "docker.run")?;
        let image = opts.get::<Option<String>>("image")?.ok_or_else(|| {
            mlua::Error::RuntimeError("docker.run requires an `image`".into())
        })?;
        let ports = Self::parse_ports(opts)?;
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
        let wait = Self::parse_wait(opts)?;
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
            files: files::parse(opts)?,
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
    match client.negotiate_version().await {
        Ok(negotiated) => Ok(negotiated),
        Err(_) => Docker::connect_with_local_defaults().map_err(derr),
    }
}

/// Pull the image only if it isn't already local — `docker run`'s own rule. A locally-BUILT
/// image (docker.build) exists in no registry, so an unconditional pull fails it with a
/// misleading "pull access denied / repository does not exist"; and for a pulled image, a
/// tag that's already present skips a pointless registry round-trip.
async fn pull_if_absent(
    client: &Docker,
    image: &str,
    progress: &super::Arc<dyn super::Progress>,
) -> mlua::Result<()> {
    if client.inspect_image(image).await.is_ok() {
        return Ok(());
    }
    // The dominant cause of a run that looks hung: a cold pull is tens of MB over a registry
    // and, until now, drained in total silence. bollard already hands us per-layer status —
    // this reports it instead of discarding it (docs/plans/run-progress-feedback.md #1).
    let activity = super::progress::start(progress, super::Kind::Pull, image.to_string());
    let (from_image, tag) = split_image(image);
    let mut pull = client.create_image(
        Some(CreateImageOptions {
            from_image,
            tag,
            ..Default::default()
        }),
        None,
        None,
    );
    // Layer ids seen, so the completion note can say how big the pull actually was. Counting
    // ids rather than stream items: bollard emits many messages per layer.
    let mut layers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    while let Some(item) = pull.next().await {
        let info = item.map_err(derr)?;
        if let Some(id) = info.id.as_deref() {
            // Docker uses the image ref itself as the `id` on summary messages; only short
            // hex layer ids are layers.
            if id.len() <= 16 && !id.contains(':') && layers.insert(id.to_string()) {
                activity.update(&format!("{} layers", layers.len()));
            }
        }
    }
    if layers.is_empty() {
        activity.done();
    } else {
        let n = layers.len();
        activity.done_with(format!("{n} layer{}", if n == 1 { "" } else { "s" }));
    }
    Ok(())
}

/// The bollard container config for a spec: each container port published to a random host port
/// (host_port "0", or the fixed one the spec names), joined to a user-defined network at create
/// time (so embedded DNS resolves the container by its alias from the first moment).
fn container_config(spec: &Spec) -> Config<String> {
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
    let networking_config = spec.network.as_ref().map(|net_name| {
        let mut endpoint = EndpointSettings::default();
        if let Some(alias) = &spec.alias {
            endpoint.aliases = Some(vec![alias.clone()]);
        }
        let mut endpoints_config = HashMap::new();
        endpoints_config.insert(net_name.clone(), endpoint);
        NetworkingConfig { endpoints_config }
    });
    Config {
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
    }
}

async fn start(
    spec: Spec,
    progress: &super::Arc<dyn super::Progress>,
) -> mlua::Result<Container> {
    let client = connect().await?;
    pull_if_absent(&client, &spec.image, progress).await?;
    let config = container_config(&spec);
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
        // BETWEEN create and start: the container must see its files at boot, and a started
        // container has already read them (docs/design/agent-ergonomics.md#containerized-mounts).
        if !spec.files.is_empty() {
            let archive = files::tar_bytes(&spec.files)?;
            client
                .upload_to_container(
                    &id,
                    Some(bollard::container::UploadToContainerOptions {
                        path: "/",
                        ..Default::default()
                    }),
                    archive.into(),
                )
                .await
                .map_err(derr)?;
        }
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
        wait_ready(&container, &wait, progress, &spec.image).await?;
    }
    Ok(container)
}

mod files;
mod readiness;
use readiness::*;

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

    /// These drive `start` directly; activity reporting is not what they are about, so they get
    /// the silent sink a library consumer gets.
    fn silent() -> crate::progress::NullProgressArc {
        crate::progress::null()
    }

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
            files: Vec::new(),
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
            .block_on(start(spec_with_fault(1), &silent()))
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
        let msg = match rt.block_on(start(spec_with_fault(99), &silent())) {
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
        let result = rt.block_on(start(spec, &silent()));

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
