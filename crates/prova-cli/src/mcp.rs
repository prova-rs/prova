//! `prova mcp` — serve Prova as an MCP stdio server (phases 2 + 3 of docs/design/mcp-mode.md).
//!
//! The server resolves the prova home / manifest / plugins ONCE at startup, exactly like the run
//! path, then exposes the CLI-parity tools over newline-delimited JSON-RPC on stdio:
//!
//!   * `run`  ↔ `prova` with the selection flags (`-k` / `--tags` / `--node` / `--last-failed`)
//!   * `list` ↔ `prova --list` (same selection)
//!   * `eval` ↔ `prova eval '<code>'`
//!
//! plus the **warm topology tools** — the MCP-only capability:
//!
//!   * `up { name }`     provision a named topology once, INSIDE the server, and hold it
//!   * `run/eval { topology }`  resolve the held instance (same live Lua values) — warm re-runs
//!   * `status {}` / `down { name }`  list what's held / run the one true teardown
//!
//! Warm threading design: an `mlua::Lua` is `!Send`, so each held topology lives on its own
//! **holder thread** that owns the `HeldTopology` (Lua state + runtime + parked teardowns) for its
//! whole life. Tool handlers talk to it over an mpsc command channel (`WarmCmd`), so provisioning,
//! warm runs, warm evals, and teardown all execute on the thread that owns the Lua. The server
//! keeps only `Send` data per holder (endpoints, the command sender, the join handle). Ownership:
//! warm runs never reap the held instance — only `down` or server shutdown (stdin EOF, which
//! hangs up every command channel and joins the holders, each tearing down on its way out) do.
//!
//! Every tool returns ONE text content item whose text is compact JSON — the stable machine
//! contract. The embedded agent skill ships as the connection's `instructions`, so an MCP agent
//! "just knows" Prova on connect. The stdio transport owns stdout: every diagnostic goes to
//! stderr, and stdin EOF is a clean shutdown (exit 0).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::home::Home;
use crate::manifest::SuiteDecl;
use crate::plugins;
use prova_core::{
    discover_files, discover_path_with, eval_snippet, hold_topology, run_suites, Endpoint, Event,
    Outcome, PortMode, Reporter, Selection, XdgSystemLayout,
};

/// `prova mcp [--profile NAME] [--manifest PATH] [-P name=source]` — resolve the environment once
/// (same home/manifest/plugins resolution as the run path), then serve until the client hangs up.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut profile: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut cli_plugins: Vec<String> = Vec::new();

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if let Some(v) = crate::value_flag(&arg, &mut it, &["--profile", "-p"]) {
            profile = Some(v);
            continue;
        }
        if let Some(v) = crate::value_flag(&arg, &mut it, &["--manifest"]) {
            manifest_path = Some(v);
            continue;
        }
        if let Some(v) = crate::value_flag(&arg, &mut it, &["--plugin", "-P"]) {
            cli_plugins.push(v);
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "usage: prova mcp [--profile NAME] [--manifest PATH] [-P name=source]\n\
                     \n\
                     serve Prova as an MCP stdio server. Tools mirror the CLI one-to-one:\n\
                     \x20 run   {{ keywords?, keyword_excludes?, tags?, tag_excludes?, nodes?,\n\
                     \x20         last_failed?, profile?, jobs? }}   ↔  prova + selection flags\n\
                     \x20 list  {{ same selection fields }}           ↔  prova --list\n\
                     \x20 eval  {{ code }}                            ↔  prova eval '<code>'\n\
                     \n\
                     the environment (home, manifest, plugins) resolves once at startup from the\n\
                     working directory, exactly like a CLI run. The embedded agent skill is served\n\
                     as the connection's instructions."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("prova mcp: unexpected argument {other:?}");
                return ExitCode::from(2);
            }
        }
    }

    let layout = match XdgSystemLayout::new() {
        Ok(layout) => layout,
        Err(err) => {
            eprintln!("prova: cannot determine home directories: {err}");
            return ExitCode::from(2);
        }
    };

    // Same home/manifest resolution as the run path. A missing manifest is fine at startup —
    // `eval` still works with the built-ins; `run`/`list` report the absence per call.
    let home: Option<Home> = if let Some(path) = &manifest_path {
        Some(crate::home::from_manifest_path(Path::new(path)))
    } else {
        match crate::home::find(Path::new(".")) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("prova: {e}");
                return ExitCode::from(2);
            }
        }
    };

    let (mut plugins_resolved, sources, paths, declared, jobs, capabilities) = match &home {
        Some(home) => {
            match crate::resolve_from_manifest(home, profile, None, None, None, &layout) {
                Ok(r) => (
                    r.plugins,
                    r.sources,
                    r.paths,
                    r.suites,
                    r.jobs,
                    r.capabilities,
                ),
                Err(code) => return code,
            }
        }
        None => (
            plugins::ResolvedPlugins::default(),
            BTreeMap::new(),
            Vec::new(),
            BTreeMap::new(),
            1,
            prova_core::Capabilities::default(),
        ),
    };
    if let Err(code) =
        crate::layer_cli_plugins(&cli_plugins, &layout, &sources, &mut plugins_resolved)
    {
        return code;
    }

    let env = Arc::new(McpEnv {
        layout,
        home,
        cli_plugins,
        paths,
        declared,
        jobs,
        plugins: plugins_resolved,
        capabilities,
    });

    // A current-thread runtime, deliberately: warm tools are stateful (up → run → down), so tool
    // side-effects must apply in the order requests arrive on stdin. On one scheduler thread each
    // dispatched request reaches the shared `run_lock` (a FIFO mutex) before the reader dispatches
    // the next, which preserves arrival order end to end. Blocking engine work still runs off-
    // thread via `spawn_blocking`.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("prova mcp: cannot start async runtime: {err}");
            return ExitCode::from(2);
        }
    };

    // The warm-topology registry outlives the connection: after stdin EOF the server is still the
    // holder of record for anything not yet `down`ed, and must reap it before exiting.
    let warm: WarmRegistry = Arc::new(Mutex::new(HashMap::new()));

    let result = runtime.block_on({
        let warm = warm.clone();
        async move {
            let service = ProvaMcpServer::new(env, warm).serve(stdio()).await?;
            // Hold until the client disconnects; stdin EOF is a clean close, hence a clean exit.
            service.waiting().await?;
            Ok::<(), Box<dyn std::error::Error>>(())
        }
    });

    // Clean shutdown = clean teardown: hang up each remaining holder's command channel (its loop
    // exits and runs the held scope's teardowns on its own thread) and wait for it to finish.
    let leftovers: Vec<(String, WarmHandle)> =
        warm.lock().expect("warm registry").drain().collect();
    for (name, handle) in leftovers {
        eprintln!("prova mcp: tearing down held topology {name:?} on shutdown");
        drop(handle.tx);
        if handle.join.join().is_err() {
            eprintln!("prova mcp: holder thread for topology {name:?} panicked during teardown");
        }
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("prova mcp: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The environment resolved once at server startup — the exact inputs a CLI run would use from
/// this working directory. Shared (read-only) across tool calls.
struct McpEnv {
    layout: XdgSystemLayout,
    home: Option<Home>,
    /// `-P name=source` server args, re-layered when a call re-resolves with a `profile`.
    cli_plugins: Vec<String>,
    /// Manifest `[run] paths` (empty when there is no manifest).
    paths: Vec<String>,
    /// Manifest `[suites.*]` declarations.
    declared: BTreeMap<String, SuiteDecl>,
    jobs: usize,
    plugins: plugins::ResolvedPlugins,
    /// Capabilities the startup project's `prova.lua` registered. Per-project calls re-resolve their
    /// own (see `CallEnv`), so two projects served by one warm server never share a vocabulary.
    capabilities: prova_core::Capabilities,
}

/// The manifest-resolved inputs one tool call runs with.
struct CallEnv {
    /// The home this call resolved against — the server's, or a `project`'s. Carried so
    /// `engine_config` roots `prova.root`/`prova.home` at the project the call actually targets.
    home: Home,
    base_dir: PathBuf,
    paths: Vec<String>,
    declared: BTreeMap<String, SuiteDecl>,
    jobs: usize,
    plugins: plugins::ResolvedPlugins,
    /// This call's registered capabilities — the startup set, or the project's own on a `project`
    /// re-resolve. Never the process's: capabilities are per-resolve, not global.
    capabilities: prova_core::Capabilities,
}

impl McpEnv {
    /// The startup resolution — or a fresh one when the call names a `profile` (the MCP analogue
    /// of `--profile`, which changes what the manifest resolves to).
    /// Locate the home a call targets: the server's startup home by default — the **affinity**, the
    /// way a shell is "in" a directory — or a caller-supplied `project`, resolved exactly as a CLI
    /// run from there would (walking up, checking each ancestor's `prova/` child).
    ///
    /// The affinity is a default, not a cage: an agent works across repos, and it *creates* projects.
    /// A supplied `project` also always resolves FRESH, which is the escape hatch for the startup
    /// snapshot — scaffold a `prova.toml` and target it in the same session, no restart.
    fn locate(&self, project: Option<&str>) -> Result<Option<Home>, String> {
        let Some(p) = project else {
            return Ok(self.home.clone());
        };
        let path = Path::new(p);
        if path.is_file() {
            return Ok(Some(crate::home::from_manifest_path(path)));
        }
        if !path.is_dir() {
            return Err(format!(
                "project {p:?} is not a directory or a manifest file"
            ));
        }
        crate::home::find(path).map_err(|e| e.to_string())
    }

    fn resolve_call(
        &self,
        profile: Option<&str>,
        project: Option<&str>,
    ) -> Result<CallEnv, String> {
        let located = self.locate(project)?;
        let home = located.as_ref().ok_or_else(|| match project {
            Some(p) => format!("no prova.toml found in {p:?} or any parent"),
            None => "no prova.toml found in this directory or any parent".to_string(),
        })?;
        // The startup snapshot is only valid for the startup home with no profile override.
        match if project.is_some() {
            Some(profile.unwrap_or_default())
        } else {
            profile
        } {
            None => Ok(CallEnv {
                home: home.clone(),
                base_dir: home.root.clone(),
                paths: self.paths.clone(),
                declared: self.declared.clone(),
                jobs: self.jobs,
                plugins: self.plugins.clone(),
                capabilities: self.capabilities.clone(),
            }),
            Some(p) => {
                let p = if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                };
                // `resolve_from_manifest` reports detail on stderr (the diagnostic channel).
                let mut run = crate::resolve_from_manifest(home, p.clone(), None, None, None, &self.layout)
                    .map_err(|_| {
                        format!(
                            "could not resolve manifest at {} (profile {p:?}) — details on the server's stderr",
                            home.manifest.display()
                        )
                    })?;
                crate::layer_cli_plugins(
                    &self.cli_plugins,
                    &self.layout,
                    &run.sources,
                    &mut run.plugins,
                )
                .map_err(|_| {
                    "could not resolve --plugin entries (details on the server's stderr)"
                        .to_string()
                })?;
                Ok(CallEnv {
                    home: home.clone(),
                    base_dir: home.root.clone(),
                    paths: run.paths,
                    declared: run.suites,
                    jobs: run.jobs,
                    plugins: run.plugins,
                    capabilities: run.capabilities,
                })
            }
        }
    }
}

/// Selection fields shared by `run` and `list` — the MCP mirror of the CLI's
/// `-k` / `--tags` / `--node` / `--last-failed` / `--profile`.
#[derive(Deserialize, JsonSchema, Default)]
struct SelectionArgs {
    /// Select nodes whose path contains any of these substrings (CLI `-k PATTERN`).
    keywords: Option<Vec<String>>,
    /// Exclude nodes whose path contains any of these substrings (CLI `-k '!PATTERN'`).
    keyword_excludes: Option<Vec<String>>,
    /// Select nodes tagged with any of these tags (CLI `--tags a,b`).
    tags: Option<Vec<String>>,
    /// Exclude nodes tagged with any of these tags (CLI `--tags '!tag'`).
    tag_excludes: Option<Vec<String>>,
    /// Select exact node paths (CLI `--node PATH`) — re-run precisely what a report named.
    nodes: Option<Vec<String>>,
    /// Also select the nodes that failed in the previous run (CLI `--last-failed`).
    last_failed: Option<bool>,
    /// Manifest profile to resolve for this call (CLI `--profile NAME`).
    profile: Option<String>,
    /// Target ANOTHER suite: a directory to resolve from (as a CLI run there would — walking up,
    /// checking each ancestor's `prova/` child), or a manifest path. Omit to use the server's
    /// startup project. A `project` always resolves fresh, so a manifest you just created or edited
    /// is picked up without restarting the server.
    project: Option<String>,
}

#[derive(Deserialize, JsonSchema, Default)]
struct RunRequest {
    #[serde(flatten)]
    selection: SelectionArgs,
    /// Run up to N suites concurrently (CLI `--jobs N`). Ignored for warm runs (one held state).
    jobs: Option<u32>,
    /// Run WARM against a topology held by a prior `up`: `t:use(<topology>)` resolves the held
    /// live instance instead of provisioning — millisecond re-runs. The topology must already be
    /// held; this never provisions implicitly.
    topology: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct EvalRequest {
    /// Lua snippet, evaluated in the full prova environment (built-in modules, declared plugins
    /// via require(), a real transient `ctx`). A bare expression or statements with `return`.
    code: String,
    /// Evaluate WARM inside a topology held by a prior `up`: the held value is available as a
    /// global named after the topology (e.g. `return orders.db.url`), and `ctx:use(<name>)`
    /// resolves the held instance. The topology must already be held.
    topology: Option<String>,
    /// Target ANOTHER suite: a directory to resolve from (as a CLI run there would — walking up,
    /// checking each ancestor's `prova/` child), or a manifest path. Omit to use the server's
    /// startup project. A `project` always resolves fresh, so a manifest you just created or edited
    /// is picked up without restarting the server.
    project: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct UpRequest {
    /// The topology to provision and hold (a `prova.topology(<name>, ...)` declared in the
    /// project's test files).
    name: String,
    /// Manifest profile to resolve for this provisioning (CLI `--profile NAME`).
    profile: Option<String>,
    /// Pin each resource to its canonical host port (CLI `prova up --fixed`) instead of a random
    /// one. Only one fixed instance of a port can be up at a time.
    fixed: Option<bool>,
    /// Target ANOTHER suite: a directory to resolve from (as a CLI run there would — walking up,
    /// checking each ancestor's `prova/` child), or a manifest path. Omit to use the server's
    /// startup project. A `project` always resolves fresh, so a manifest you just created or edited
    /// is picked up without restarting the server.
    project: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct DownRequest {
    /// The held topology to tear down.
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct IntrospectRequest {
    /// Case-insensitive substring, matched across name and summary — `"shell"`, `"retry"`,
    /// `"tempdir"`. Omit for the whole surface.
    filter: Option<String>,
}

fn to_selection(args: &SelectionArgs) -> Selection {
    Selection {
        keywords: args.keywords.clone().unwrap_or_default(),
        keyword_excludes: args.keyword_excludes.clone().unwrap_or_default(),
        tags: args.tags.clone().unwrap_or_default(),
        tag_excludes: args.tag_excludes.clone().unwrap_or_default(),
        nodes: args.nodes.clone().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------------------------
// Warm topology holding — one holder thread per held topology (see the module doc)
// ---------------------------------------------------------------------------------------------

/// The server-side registry of held topologies: name → the `Send` half of its holder (endpoints
/// for `up`/`status` results, the command channel, and the join handle for `down`/shutdown).
type WarmRegistry = Arc<Mutex<HashMap<String, WarmHandle>>>;

struct WarmHandle {
    endpoints: Vec<Endpoint>,
    tx: std::sync::mpsc::Sender<WarmCmd>,
    join: std::thread::JoinHandle<()>,
}

/// A command executed on the holder thread — the thread that owns the topology's Lua state.
enum WarmCmd {
    Run {
        selection: Selection,
        reply: std::sync::mpsc::Sender<Result<WarmRunOutcome, String>>,
    },
    Eval {
        code: String,
        reply: std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
    },
    /// Tear the held topology down and exit the holder thread. The reply confirms teardown
    /// *completed* before the `down` tool returns.
    Down { reply: std::sync::mpsc::Sender<()> },
}

/// A warm run's owned results, shaped to cross the holder→handler channel.
struct WarmRunOutcome {
    passed: usize,
    failed: usize,
    skipped: usize,
    deselected: usize,
    duration_ms: u64,
    failures: Vec<(String, String)>,
}

/// The holder thread's whole life: provision the topology (reporting readiness or the error over
/// `ready`), then serve warm commands until `Down` arrives or every sender hangs up (server
/// shutdown) — either way the held scope's teardown runs HERE, on the thread that owns the Lua.
fn warm_holder(
    files: Vec<PathBuf>,
    name: String,
    config: prova_core::RunConfig,
    ready: std::sync::mpsc::Sender<Result<Vec<Endpoint>, String>>,
    cmds: std::sync::mpsc::Receiver<WarmCmd>,
) {
    let held = match hold_topology(&files, &name, &config) {
        Ok(held) => held,
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return; // a failed provisioning already tore its partial resources down
        }
    };
    let _ = ready.send(Ok(held.endpoints().to_vec()));

    let mut down_reply = None;
    while let Ok(cmd) = cmds.recv() {
        match cmd {
            WarmCmd::Run { selection, reply } => {
                let mut collector = FailureCollector::default();
                let outcome = held
                    .run_warm(&files, &selection, &mut collector)
                    .map(|summary| WarmRunOutcome {
                        passed: summary.passed,
                        failed: summary.failed,
                        skipped: summary.skipped,
                        deselected: summary.deselected,
                        duration_ms: summary.duration.as_millis() as u64,
                        failures: collector.failures,
                    })
                    .map_err(|e| e.to_string());
                let _ = reply.send(outcome);
            }
            WarmCmd::Eval { code, reply } => {
                let _ = reply.send(held.eval_warm(&code).map_err(|e| e.to_string()));
            }
            WarmCmd::Down { reply } => {
                down_reply = Some(reply);
                break;
            }
        }
    }
    // The one true teardown — explicit `down`, or every sender gone (server shutdown).
    held.teardown();
    if let Some(reply) = down_reply {
        let _ = reply.send(());
    }
}

/// The files that may declare topologies: every test file under the manifest's run paths and any
/// explicit suite paths — the exact discovery `prova up` uses (`build_topology_run` in main.rs),
/// so the two holders consume one definition the same way. Warm runs re-run this same set.
fn topology_files(call: &CallEnv) -> Result<Vec<PathBuf>, String> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut discover = |rel: &str| -> Result<(), String> {
        let found = discover_files(&call.base_dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        files.extend(found);
        Ok(())
    };
    for p in &call.paths {
        discover(p)?;
    }
    for decl in call.declared.values() {
        for p in &decl.paths {
            discover(p)?;
        }
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err(
            "no files found to search for topologies (looked for *_test.lua / *.test.lua)".into(),
        );
    }
    Ok(files)
}

#[derive(Clone)]
pub struct ProvaMcpServer {
    env: Arc<McpEnv>,
    /// Held topologies (warm mode), keyed by name. Owned outside the server too (see `run`), so
    /// stdin EOF can reap whatever is still held.
    warm: WarmRegistry,
    /// Suite runs mutate shared project state (the `--last-failed` file, snapshots), and the warm
    /// tools are stateful across calls (`up` → `run{topology}` → `down`), so EVERY tool serializes
    /// through this FIFO mutex — on the current-thread runtime that also pins tool side-effects to
    /// stdin arrival order (the CLI serializes by being one process per run).
    run_lock: Arc<tokio::sync::Mutex<()>>,
    // Read through the `#[tool_handler]` macro on the ServerHandler impl — dead-code analysis
    // can't see past the macro expansion.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_handler]
impl ServerHandler for ProvaMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("prova", env!("CARGO_PKG_VERSION")))
            // The embedded agent skill — one document, every transport (see `prova skill`).
            .with_instructions(crate::SKILL)
    }
}

#[tool_router]
impl ProvaMcpServer {
    fn new(env: Arc<McpEnv>, warm: WarmRegistry) -> Self {
        Self {
            env,
            warm,
            run_lock: Arc::new(tokio::sync::Mutex::new(())),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "run",
        description = "Run the project's test suite with an optional selection (the MCP mirror of the CLI's -k/--tags/--node/--last-failed/--profile/--jobs flags). With `topology`, run WARM against a topology held by a prior `up`: t:use resolves the held live instance instead of provisioning (never provisions implicitly — an un-held topology is an error). Returns compact JSON: { passed, failed, skipped, deselected, duration_ms, failures: [{ path, message }] }. The result is marked isError when any node failed. Also records the failed nodes so a later run with last_failed=true (or CLI --last-failed) re-runs exactly them. Pass `project` (a directory or manifest path) to target ANOTHER suite anywhere on disk — the server's startup project is only the default, and a `project` resolves fresh, so a manifest you just scaffolded works without a restart."
    )]
    async fn run(&self, Parameters(req): Parameters<RunRequest>) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let env = self.env.clone();
        if let Some(topology) = req.topology.clone() {
            let warm = self.warm.clone();
            return blocking(move || warm_run_blocking(&env, &warm, &topology, req)).await;
        }
        blocking(move || run_blocking(&env, req)).await
    }

    #[tool(
        name = "list",
        description = "Discover the project's test nodes without running them (the MCP mirror of `prova --list`), honoring the same selection fields as `run`. Returns compact JSON: { nodes: [{ path }] }. Pass `project` (a directory or manifest path) to target ANOTHER suite anywhere on disk — the server's startup project is only the default, and a `project` resolves fresh, so a manifest you just scaffolded works without a restart."
    )]
    async fn list(&self, Parameters(req): Parameters<SelectionArgs>) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let env = self.env.clone();
        blocking(move || list_blocking(&env, req)).await
    }

    #[tool(
        name = "introspect",
        description = "Discover prova's API surface WITHOUT reading its source: every function, method, and value shape as { name, signature, summary }. `filter` narrows by substring across name+summary (e.g. \"shell\", \"tempdir\", \"ShellResult\"). Start here — it answers what to call, how to call it, and what comes back, so you don't have to probe with eval. Parsed from the same LuaCATS stubs that drive editor completion, so it cannot drift from what an author sees."
    )]
    async fn introspect(&self, Parameters(req): Parameters<IntrospectRequest>) -> CallToolResult {
        // No Lua environment needed — the surface is static, so introspection never provisions,
        // never blocks on a run, and works before a manifest exists.
        let all = prova_core::help::core_entries();
        let entries = match req.filter.as_deref().map(str::trim) {
            Some(n) if !n.is_empty() => prova_core::help::filter(&all, n),
            _ => all,
        };
        let rows: Vec<_> = entries
            .iter()
            .map(|e| json!({ "name": e.name, "signature": e.signature, "summary": e.summary }))
            .collect();
        CallToolResult::success(vec![Content::text(json!({ "entries": rows }).to_string())])
    }

    #[tool(
        name = "eval",
        description = "Evaluate a one-shot Lua snippet in the full prova environment (built-in modules like fs/shell/docker/http, manifest-declared plugins via require(), a real transient ctx torn down afterwards) — the MCP mirror of `prova eval`. With `topology`, evaluate WARM inside a held topology: its value is a global named after it (e.g. `return orders.db.url`). Returns the snippet's returned value as compact JSON. A raising snippet returns an error result carrying the Lua error. Pass `project` (a directory or manifest path) to target ANOTHER suite anywhere on disk — the server's startup project is only the default, and a `project` resolves fresh, so a manifest you just scaffolded works without a restart."
    )]
    async fn eval(&self, Parameters(req): Parameters<EvalRequest>) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let env = self.env.clone();
        if let Some(topology) = req.topology.clone() {
            let warm = self.warm.clone();
            return blocking(move || warm_eval_blocking(&warm, &topology, req.code)).await;
        }
        blocking(move || eval_blocking(&env, req.code, req.project)).await
    }

    #[tool(
        name = "up",
        description = "Provision a named topology (a prova.topology declaration) INSIDE the server and hold it across tool calls — the warm holder. The factory runs exactly once; subsequent run/eval calls with `topology` resolve the held live instance. Returns compact JSON: { name, resources: [{ name, url }] }. Tear it down with `down` (or server shutdown). A held environment accumulates state — down + up when isolation matters. Pass `project` (a directory or manifest path) to target ANOTHER suite anywhere on disk — the server's startup project is only the default, and a `project` resolves fresh, so a manifest you just scaffolded works without a restart."
    )]
    async fn up(&self, Parameters(req): Parameters<UpRequest>) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let env = self.env.clone();
        let warm = self.warm.clone();
        blocking(move || up_blocking(&env, &warm, req)).await
    }

    #[tool(
        name = "down",
        description = "Tear down a topology held by `up`: runs the held scope's teardowns (ctx:defer/ctx:manage, LIFO) and releases it. The holder is the ONLY reaper — warm runs never tear the held instance down. Returns compact JSON: { name, down: true }."
    )]
    async fn down(&self, Parameters(req): Parameters<DownRequest>) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let warm = self.warm.clone();
        blocking(move || down_blocking(&warm, &req.name)).await
    }

    #[tool(
        name = "status",
        description = "List the topologies currently held by `up` (warm mode). Returns compact JSON: { held: [{ name, resources: [{ name, url }] }] }."
    )]
    async fn status(&self) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let held: Vec<serde_json::Value> = {
            let registry = self.warm.lock().expect("warm registry");
            let mut names: Vec<&String> = registry.keys().collect();
            names.sort();
            names
                .into_iter()
                .map(|name| {
                    let handle = &registry[name];
                    json!({ "name": name, "resources": endpoints_json(&handle.endpoints) })
                })
                .collect()
        };
        CallToolResult::success(vec![Content::text(json!({ "held": held }).to_string())])
    }
}

/// Endpoints as the `resources` JSON array every warm result shape shares.
fn endpoints_json(endpoints: &[Endpoint]) -> Vec<serde_json::Value> {
    endpoints
        .iter()
        .map(|e| json!({ "name": e.name, "url": e.url }))
        .collect()
}

/// Run a blocking engine call off the async executor (the engine builds its own runtimes
/// internally — it must never block rmcp's executor) and shape the outcome as a tool result:
/// `Ok((json, is_error))` becomes one compact-JSON text content item; `Err` an error text result.
async fn blocking<F>(f: F) -> CallToolResult
where
    F: FnOnce() -> Result<(serde_json::Value, bool), String> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok((value, is_error))) => {
            let content = vec![Content::text(value.to_string())];
            if is_error {
                CallToolResult::error(content)
            } else {
                CallToolResult::success(content)
            }
        }
        Ok(Err(message)) => CallToolResult::error(vec![Content::text(message)]),
        Err(join) => CallToolResult::error(vec![Content::text(format!(
            "prova mcp: tool task failed: {join}"
        ))]),
    }
}

/// Collects each failed node's (path, message) — the per-failure detail in the `run` result, and
/// the state the next `last_failed` selection re-runs.
#[derive(Default)]
struct FailureCollector {
    failures: Vec<(String, String)>,
}

impl Reporter for FailureCollector {
    fn event(&mut self, event: &Event) {
        if let Event::NodeFinished {
            path,
            outcome: Outcome::Failed,
            message,
            ..
        } = event
        {
            self.failures
                .push((path.to_string(), message.unwrap_or("").to_string()));
        }
    }
}

fn run_blocking(env: &McpEnv, req: RunRequest) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(
        req.selection.profile.as_deref(),
        req.selection.project.as_deref(),
    )?;

    let mut selection = to_selection(&req.selection);
    // `last_failed`: fold the previous run's failed node paths in, exactly like `--last-failed`.
    if req.selection.last_failed.unwrap_or(false) {
        match crate::load_last_failed(&env.home) {
            Some(paths) if !paths.is_empty() => selection.nodes.extend(paths),
            _ => eprintln!(
                "prova mcp: last_failed: no failure state from a previous run here; running everything"
            ),
        }
    }

    let suites = crate::collect_suites(&call.base_dir, &call.declared, &call.paths)?;
    if suites.is_empty() {
        return Err("no test files found (looked for *_test.lua / *.test.lua)".into());
    }

    let jobs = req.jobs.map(|n| (n as usize).max(1)).unwrap_or(call.jobs);
    let mut config = crate::engine_config(jobs, &env.layout, &call.plugins, Some(&call.home))
        .with_capabilities(call.capabilities.clone());
    config.selection = selection;

    let mut reporter = FailureCollector::default();
    let summary = run_suites(&suites, &mut reporter, &config).map_err(|e| e.to_string())?;

    // Keep the `--last-failed` state in step with CLI runs — the two transports share one loop.
    let failed_paths: Vec<String> = reporter.failures.iter().map(|(p, _)| p.clone()).collect();
    crate::store_last_failed(&env.home, &failed_paths);

    let failures: Vec<serde_json::Value> = reporter
        .failures
        .iter()
        .map(|(path, message)| json!({ "path": path, "message": message }))
        .collect();
    let result = json!({
        "passed": summary.passed,
        "failed": summary.failed,
        "skipped": summary.skipped,
        "deselected": summary.deselected,
        "duration_ms": summary.duration.as_millis() as u64,
        "failures": failures,
    });
    Ok((result, summary.failed > 0))
}

fn list_blocking(env: &McpEnv, req: SelectionArgs) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(req.profile.as_deref(), req.project.as_deref())?;

    let mut selection = to_selection(&req);
    if req.last_failed.unwrap_or(false) {
        if let Some(paths) = crate::load_last_failed(&env.home) {
            selection.nodes.extend(paths);
        }
    }

    let suites = crate::collect_suites(&call.base_dir, &call.declared, &call.paths)?;
    let mut config = crate::engine_config(1, &env.layout, &call.plugins, Some(&call.home))
        .with_capabilities(call.capabilities.clone());
    config.selection = selection;

    let mut nodes: Vec<serde_json::Value> = Vec::new();
    for file in suites.iter().flat_map(|s| &s.files) {
        let node_paths =
            discover_path_with(file, &config).map_err(|e| format!("{}: {e}", file.display()))?;
        nodes.extend(node_paths.into_iter().map(|p| json!({ "path": p })));
    }
    Ok((json!({ "nodes": nodes }), false))
}

fn eval_blocking(
    env: &McpEnv,
    code: String,
    project: Option<String>,
) -> Result<(serde_json::Value, bool), String> {
    if code.trim().is_empty() {
        return Err("eval: the snippet is empty".into());
    }
    // `eval` deliberately works with NO manifest (the built-ins alone are useful), so it cannot go
    // through `resolve_call`, which requires one. A `project` still targets another suite: resolve
    // its home + plugins so `require(...)` and `prova.root` mean what they mean *there*.
    let (home, plugins) = match project.as_deref() {
        None => (env.home.clone(), env.plugins.clone()),
        Some(p) => {
            let call = env.resolve_call(None, Some(p))?;
            (Some(call.home), call.plugins)
        }
    };
    let config = crate::engine_config(1, &env.layout, &plugins, home.as_ref());
    eval_snippet(&code, &config)
        .map(|value| (value, false))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Warm tool bodies (each runs under `blocking`, talking to a holder thread where needed)
// ---------------------------------------------------------------------------------------------

fn up_blocking(
    env: &McpEnv,
    warm: &WarmRegistry,
    req: UpRequest,
) -> Result<(serde_json::Value, bool), String> {
    let name = req.name;
    if warm.lock().expect("warm registry").contains_key(&name) {
        return Err(format!(
            "topology {name:?} is already up — `down` it first (a held environment accumulates \
             state; down + up is the reset)"
        ));
    }

    let call = env.resolve_call(req.profile.as_deref(), req.project.as_deref())?;
    let files = topology_files(&call)?;
    let config = crate::engine_config(1, &env.layout, &call.plugins, Some(&call.home))
        .with_capabilities(call.capabilities.clone())
        .with_ports(if req.fixed.unwrap_or(false) {
            PortMode::Fixed
        } else {
            PortMode::Auto
        });

    // Spawn the holder thread; it owns the Lua state for this topology's whole held life.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let thread_name = name.clone();
    let join = std::thread::Builder::new()
        .name(format!("prova-warm-{name}"))
        .spawn(move || warm_holder(files, thread_name, config, ready_tx, cmd_rx))
        .map_err(|e| format!("cannot spawn the holder thread: {e}"))?;

    match ready_rx.recv() {
        Ok(Ok(endpoints)) => {
            let resources = endpoints_json(&endpoints);
            warm.lock().expect("warm registry").insert(
                name.clone(),
                WarmHandle {
                    endpoints,
                    tx: cmd_tx,
                    join,
                },
            );
            Ok((json!({ "name": name, "resources": resources }), false))
        }
        Ok(Err(message)) => {
            let _ = join.join(); // the holder already tore down its partial resources
            Err(format!("up {name:?}: {message}"))
        }
        Err(_) => {
            let _ = join.join();
            Err(format!(
                "up {name:?}: the holder thread exited unexpectedly"
            ))
        }
    }
}

fn down_blocking(warm: &WarmRegistry, name: &str) -> Result<(serde_json::Value, bool), String> {
    let handle = warm
        .lock()
        .expect("warm registry")
        .remove(name)
        .ok_or_else(|| format!("topology {name:?} is not held (see `status`)"))?;

    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    let sent = handle.tx.send(WarmCmd::Down { reply: reply_tx }).is_ok();
    // Wait for the teardown to actually complete before reporting it down. A dead holder (send or
    // recv failing) still gets joined so nothing leaks, but is reported.
    let confirmed = sent && reply_rx.recv().is_ok();
    let _ = handle.join.join();
    if !confirmed {
        return Err(format!(
            "down {name:?}: the holder thread had already exited; teardown state is unknown"
        ));
    }
    Ok((json!({ "name": name, "down": true }), false))
}

/// A warm run: resolve the holder for `topology` (an un-held name is an explicit error — warm runs
/// NEVER provision implicitly) and execute the run on its thread, where the Lua lives.
fn warm_run_blocking(
    env: &McpEnv,
    warm: &WarmRegistry,
    topology: &str,
    req: RunRequest,
) -> Result<(serde_json::Value, bool), String> {
    let tx = warm
        .lock()
        .expect("warm registry")
        .get(topology)
        .map(|h| h.tx.clone())
        .ok_or_else(|| not_held(topology))?;

    let mut selection = to_selection(&req.selection);
    if req.selection.last_failed.unwrap_or(false) {
        match crate::load_last_failed(&env.home) {
            Some(paths) if !paths.is_empty() => selection.nodes.extend(paths),
            _ => eprintln!(
                "prova mcp: last_failed: no failure state from a previous run here; running everything"
            ),
        }
    }

    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    tx.send(WarmCmd::Run {
        selection,
        reply: reply_tx,
    })
    .map_err(|_| not_held(topology))?;
    let outcome = reply_rx.recv().map_err(|_| not_held(topology))??;

    // Keep the `--last-failed` state in step with cold runs — every transport shares one loop.
    let failed_paths: Vec<String> = outcome.failures.iter().map(|(p, _)| p.clone()).collect();
    crate::store_last_failed(&env.home, &failed_paths);

    let failures: Vec<serde_json::Value> = outcome
        .failures
        .iter()
        .map(|(path, message)| json!({ "path": path, "message": message }))
        .collect();
    let result = json!({
        "passed": outcome.passed,
        "failed": outcome.failed,
        "skipped": outcome.skipped,
        "deselected": outcome.deselected,
        "duration_ms": outcome.duration_ms,
        "failures": failures,
        "topology": topology,
    });
    Ok((result, outcome.failed > 0))
}

fn warm_eval_blocking(
    warm: &WarmRegistry,
    topology: &str,
    code: String,
) -> Result<(serde_json::Value, bool), String> {
    if code.trim().is_empty() {
        return Err("eval: the snippet is empty".into());
    }
    let tx = warm
        .lock()
        .expect("warm registry")
        .get(topology)
        .map(|h| h.tx.clone())
        .ok_or_else(|| not_held(topology))?;

    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    tx.send(WarmCmd::Eval {
        code,
        reply: reply_tx,
    })
    .map_err(|_| not_held(topology))?;
    let value = reply_rx.recv().map_err(|_| not_held(topology))??;
    Ok((value, false))
}

/// The explicit not-held error the warm contract demands (no silent cold provisioning).
fn not_held(topology: &str) -> String {
    format!(
        "topology {topology:?} is not held — call up {{ name = {topology:?} }} first \
         (warm run/eval never provision implicitly)"
    )
}
