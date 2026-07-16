//! `prova mcp` — serve Prova as a cold MCP stdio server (phase 2 of docs/design/mcp-mode.md).
//!
//! The server resolves the prova home / manifest / plugins ONCE at startup, exactly like the run
//! path, then exposes the CLI-parity tools over newline-delimited JSON-RPC on stdio:
//!
//!   * `run`  ↔ `prova` with the selection flags (`-k` / `--tags` / `--node` / `--last-failed`)
//!   * `list` ↔ `prova --list` (same selection)
//!   * `eval` ↔ `prova eval '<code>'`
//!
//! Every tool returns ONE text content item whose text is compact JSON — the stable machine
//! contract. The embedded agent skill ships as the connection's `instructions`, so an MCP agent
//! "just knows" Prova on connect. The stdio transport owns stdout: every diagnostic goes to
//! stderr, and stdin EOF is a clean shutdown (exit 0). Warm topology tools are the next phase.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::home::Home;
use crate::manifest::SuiteDecl;
use crate::plugins;
use prova_core::{
    discover_path_with, eval_snippet, run_suites, Event, Outcome, Reporter, Selection,
    XdgSystemLayout,
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

    let (mut plugins_resolved, sources, paths, declared, jobs) = match &home {
        Some(home) => match crate::resolve_from_manifest(home, profile, None, None, &layout) {
            Ok(r) => (r.plugins, r.sources, r.paths, r.suites, r.jobs),
            Err(code) => return code,
        },
        None => (
            plugins::ResolvedPlugins::default(),
            BTreeMap::new(),
            Vec::new(),
            BTreeMap::new(),
            1,
        ),
    };
    if let Err(code) = crate::layer_cli_plugins(&cli_plugins, &layout, &sources, &mut plugins_resolved)
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
    });

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("prova mcp: cannot start async runtime: {err}");
            return ExitCode::from(2);
        }
    };

    let result = runtime.block_on(async move {
        let service = ProvaMcpServer::new(env).serve(stdio()).await?;
        // Hold until the client disconnects; stdin EOF is a clean close, hence a clean exit.
        service.waiting().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
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
}

/// The manifest-resolved inputs one tool call runs with.
struct CallEnv {
    base_dir: PathBuf,
    paths: Vec<String>,
    declared: BTreeMap<String, SuiteDecl>,
    jobs: usize,
    plugins: plugins::ResolvedPlugins,
}

impl McpEnv {
    /// The startup resolution — or a fresh one when the call names a `profile` (the MCP analogue
    /// of `--profile`, which changes what the manifest resolves to).
    fn resolve_call(&self, profile: Option<&str>) -> Result<CallEnv, String> {
        let home = self
            .home
            .as_ref()
            .ok_or("no prova.toml found in this directory or any parent")?;
        match profile {
            None => Ok(CallEnv {
                base_dir: home.dir.clone(),
                paths: self.paths.clone(),
                declared: self.declared.clone(),
                jobs: self.jobs,
                plugins: self.plugins.clone(),
            }),
            Some(p) => {
                // `resolve_from_manifest` reports detail on stderr (the diagnostic channel).
                let mut run = crate::resolve_from_manifest(
                    home,
                    Some(p.to_string()),
                    None,
                    None,
                    &self.layout,
                )
                .map_err(|_| {
                    format!("could not resolve profile {p:?} (details on the server's stderr)")
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
                    base_dir: home.dir.clone(),
                    paths: run.paths,
                    declared: run.suites,
                    jobs: run.jobs,
                    plugins: run.plugins,
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
}

#[derive(Deserialize, JsonSchema, Default)]
struct RunRequest {
    #[serde(flatten)]
    selection: SelectionArgs,
    /// Run up to N suites concurrently (CLI `--jobs N`).
    jobs: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct EvalRequest {
    /// Lua snippet, evaluated in the full prova environment (built-in modules, declared plugins
    /// via require(), a real transient `ctx`). A bare expression or statements with `return`.
    code: String,
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

#[derive(Clone)]
pub struct ProvaMcpServer {
    env: Arc<McpEnv>,
    /// Suite runs mutate shared project state (the `--last-failed` file, snapshots), so `run`
    /// calls are serialized — rmcp handles requests concurrently, but two suite runs interleaving
    /// on one project's filesystem would corrupt each other (the CLI serializes by being one
    /// process per run).
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
    fn new(env: Arc<McpEnv>) -> Self {
        Self {
            env,
            run_lock: Arc::new(tokio::sync::Mutex::new(())),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "run",
        description = "Run the project's test suite with an optional selection (the MCP mirror of the CLI's -k/--tags/--node/--last-failed/--profile/--jobs flags). Returns compact JSON: { passed, failed, skipped, deselected, duration_ms, failures: [{ path, message }] }. The result is marked isError when any node failed. Also records the failed nodes so a later run with last_failed=true (or CLI --last-failed) re-runs exactly them."
    )]
    async fn run(&self, Parameters(req): Parameters<RunRequest>) -> CallToolResult {
        let _serialized = self.run_lock.lock().await;
        let env = self.env.clone();
        blocking(move || run_blocking(&env, req)).await
    }

    #[tool(
        name = "list",
        description = "Discover the project's test nodes without running them (the MCP mirror of `prova --list`), honoring the same selection fields as `run`. Returns compact JSON: { nodes: [{ path }] }."
    )]
    async fn list(&self, Parameters(req): Parameters<SelectionArgs>) -> CallToolResult {
        let env = self.env.clone();
        blocking(move || list_blocking(&env, req)).await
    }

    #[tool(
        name = "eval",
        description = "Evaluate a one-shot Lua snippet in the full prova environment (built-in modules like fs/shell/docker/http, manifest-declared plugins via require(), a real transient ctx torn down afterwards) — the MCP mirror of `prova eval`. Returns the snippet's returned value as compact JSON. A raising snippet returns an error result carrying the Lua error."
    )]
    async fn eval(&self, Parameters(req): Parameters<EvalRequest>) -> CallToolResult {
        let env = self.env.clone();
        blocking(move || eval_blocking(&env, req.code)).await
    }
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
    let call = env.resolve_call(req.selection.profile.as_deref())?;

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
    let mut config = crate::engine_config(jobs, &env.layout, &call.plugins);
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
    let call = env.resolve_call(req.profile.as_deref())?;

    let mut selection = to_selection(&req);
    if req.last_failed.unwrap_or(false) {
        if let Some(paths) = crate::load_last_failed(&env.home) {
            selection.nodes.extend(paths);
        }
    }

    let suites = crate::collect_suites(&call.base_dir, &call.declared, &call.paths)?;
    let mut config = crate::engine_config(1, &env.layout, &call.plugins);
    config.selection = selection;

    let mut nodes: Vec<serde_json::Value> = Vec::new();
    for file in suites.iter().flat_map(|s| &s.files) {
        let node_paths =
            discover_path_with(file, &config).map_err(|e| format!("{}: {e}", file.display()))?;
        nodes.extend(node_paths.into_iter().map(|p| json!({ "path": p })));
    }
    Ok((json!({ "nodes": nodes }), false))
}

fn eval_blocking(env: &McpEnv, code: String) -> Result<(serde_json::Value, bool), String> {
    if code.trim().is_empty() {
        return Err("eval: the snippet is empty".into());
    }
    let config = crate::engine_config(1, &env.layout, &env.plugins);
    eval_snippet(&code, &config)
        .map(|value| (value, false))
        .map_err(|e| e.to_string())
}
