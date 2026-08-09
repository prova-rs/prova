//! The `*_blocking` tool bodies — the synchronous work each MCP tool wraps in `blocking(...)`.
//! Split out of `mcp.rs` to keep it under the file-size gate and leave room for new tools
//! (query consolidation, increment 8). A child module, so `use super::*` sees `mcp.rs`'s
//! private types (McpEnv, the request structs, the warm registry, to_selection, …); only the
//! entry fns the tool methods call are `pub(super)`.
#![allow(clippy::result_large_err)]

use super::*;

pub(super) fn run_blocking(env: &McpEnv, req: RunRequest) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(
        req.selection.profile.as_deref(),
        req.selection.package.as_deref(),
    )?;

    let mut selection = to_selection(&req.selection);
    // `last_failed`: fold the previous run's failed node paths in, exactly like `--last-failed`.
    // State lives in the home the call RESOLVED to (`call.home`) — a `package` call must read and
    // write that package's state, not the server's startup affinity.
    let lf_home = Some(call.home.clone());
    let mut note: Option<String> = None;
    if req.selection.last_failed.unwrap_or(false) {
        match crate::load_last_failed(&lf_home) {
            Some(paths) if !paths.is_empty() => selection.nodes.extend(paths),
            // Over MCP stderr is invisible — carry the fallback in the result, or the caller
            // cannot tell "re-ran the red set" from "ran everything".
            _ => {
                note = Some(
                    "last_failed: no failure state from a previous run here; ran everything"
                        .to_string(),
                )
            }
        }
    }

    let suites = crate::collect_suites(&call.base_dir, &call.declared, &call.proofs, true)?;
    if suites.is_empty() {
        // The same explanation the CLI gives — an agent hits this exact wall, and a bare "no
        // declaration files found" sends it hunting for a bug that is really a layout question.
        let base = "no declaration files found (looked for *.prova.lua, plus the accepted *_test.lua / *.test.lua)".to_string();
        return Err(match crate::stray_proof_hint(&call.base_dir, &call.proofs) {
            Some(hint) => format!("{base}\n{hint}"),
            None => base,
        });
    }

    let jobs = req.jobs.map(|n| (n as usize).max(1)).unwrap_or(call.jobs);
    let mut config = crate::engine_config(jobs, &call.dependencies, Some(&call.home), prova_core::progress::null())
        .with_capabilities(call.capabilities.clone())
        .with_promises_only(req.selection.promises.unwrap_or(false))
        .with_falsify(req.selection.falsify.unwrap_or(false))
        .with_due(req.due.unwrap_or(false));
    config.selection = selection;

    let mut reporter = FailureCollector::default();
    let summary = run_suites(&suites, &mut reporter, &config).map_err(|e| e.to_string())?;

    // The CLI's empty-selection contract, mirrored: a selection that matched NOTHING is an error,
    // not a green run — it usually means a typo, and a typo must not read as success. Open
    // promises count as matched, exactly as on the CLI.
    let ran = summary.passed + summary.failed + summary.skipped + summary.promised;
    if ran == 0 && !config.selection.is_empty() {
        return Err(format!(
            "selection matched no tests ({} deselected) — usually a typo; loosen the selection \
             or check `list`",
            summary.deselected
        ));
    }

    // Keep the `--last-failed` state in step with CLI runs — the two transports share one loop.
    let failed_paths: Vec<String> = reporter.failures.iter().map(|f| f.path.clone()).collect();
    crate::store_last_failed(&lf_home, &failed_paths);

    let failures: Vec<serde_json::Value> = reporter.failures.iter().map(failure_json).collect();
    let mut result = json!({
        "passed": summary.passed,
        "failed": summary.failed,
        "skipped": summary.skipped,
        "promised": summary.promised,
        "deselected": summary.deselected,
        "duration_ms": summary.duration.as_millis() as u64,
        "failures": failures,
    });
    if let Some(n) = note {
        result["note"] = json!(n);
    }
    Ok((result, summary.failed > 0))
}

/// `attest` — reconcile one obligation against the last run's record.
///
/// Marked `isError` whenever the obligation is not attested. An agent skimming tool results reads
/// the error marker long before it reads a field, and "not attested" is exactly the outcome that
/// must not be skimmed past — that is the whole failure mode this atom exists for.
pub(super) fn attest_blocking(env: &McpEnv, req: AttestRequest) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(None, req.package.as_deref())?;
    let manifest = std::fs::read_to_string(&call.home.manifest)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::manifest::Manifest::parse(&text))?;

    // A bare id resolves when exactly one claim carries it — the same rule as the CLI, so the
    // two surfaces cannot disagree about what an address means. Zero matches falls through
    // untouched (a ticket address has no `#` and no anchor, and must keep working).
    let address = if !req.address.contains('#') {
        let docs = manifest.specs.as_ref().map(|s| s.scan_roots()).unwrap_or_default();
        let scanned = crate::claims::scan(&call.home.dir, &docs).map_err(|e| e.to_string())?;
        let matches = crate::claims::matching_id(&scanned, &req.address);
        match matches.len() {
            1 => matches[0].address.clone(),
            0 => req.address.clone(),
            _ => {
                return Ok((
                    json!({
                        "address": req.address,
                        "attested": false,
                        "verdict": "ambiguous",
                        "candidates": matches.iter().map(|m| m.address.clone()).collect::<Vec<_>>(),
                    }),
                    true,
                ));
            }
        }
    } else {
        req.address.clone()
    };

    let Some(recorded) = crate::record::load(&call.home) else {
        return Ok((
            json!({
                "address": address,
                "attested": false,
                "verdict": "no_evidence",
                "reason": "no run has been recorded for this package — run the suite first",
            }),
            true,
        ));
    };

    let proofs = crate::collect_obligations(&call.home, &manifest, &call.dependencies)?;
    let bindings: Vec<String> = proofs
        .iter()
        .filter(|p| {
            p.covers
                .iter()
                .any(|c| crate::claims::split_pin(c).0 == address)
        })
        .map(|p| p.path.clone())
        .collect();

    let verdict = crate::record::attest(&recorded, &bindings);
    let attested = verdict.is_attested();
    let body = match &verdict {
        crate::record::Attested::Yes { path } => json!({
            "address": address, "attested": true, "verdict": "attested", "proof": path,
        }),
        crate::record::Attested::Red { path, outcome } => json!({
            "address": address, "attested": false, "verdict": "red", "proof": path,
            "reason": match outcome {
                crate::record::Executed::Failed => "the covering proof ran and failed",
                crate::record::Executed::Promised => "the covering proof is an open promise, red by definition",
                crate::record::Executed::Passed => "unreachable",
            },
        }),
        crate::record::Attested::NoEvidence { path, why } => json!({
            "address": address, "attested": false, "verdict": "no_evidence", "proof": path,
            "reason": why,
        }),
        crate::record::Attested::Unbound => json!({
            "address": address, "attested": false, "verdict": "unbound",
            "reason": "no proof declares `covers` for this address",
        }),
    };
    Ok((body, !attested))
}

/// `evidence` — the whole account, through the same computation as the CLI verb.
pub(super) fn evidence_blocking(env: &McpEnv, req: EvidenceRequest) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(None, req.package.as_deref())?;
    let manifest = std::fs::read_to_string(&call.home.manifest)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::manifest::Manifest::parse(&text))?;
    let account = crate::evidence_account(&call.home, &manifest, &call.dependencies)?;
    Ok((
        json!({
            "claimed": account.claimed,
            "bound": account.bound,
            "promised": account.promised,
            "attested": account.attested,
            "owed": owed_rows(&account.owed),
        }),
        false,
    ))
}

/// `owed` — the debts alone, same reconciliation, worst-first.
pub(super) fn owed_blocking(env: &McpEnv, req: OwedRequest) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(None, req.package.as_deref())?;
    let manifest = std::fs::read_to_string(&call.home.manifest)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::manifest::Manifest::parse(&text))?;
    let account = crate::evidence_account(&call.home, &manifest, &call.dependencies)?;
    Ok((json!({ "owed": owed_rows(&account.owed) }), false))
}

/// One JSON shape for a debt row, shared by `evidence` and `owed` so the two cannot drift.
fn owed_rows(owed: &[crate::claims::Owed]) -> Vec<serde_json::Value> {
    owed.iter()
        .map(|o| json!({ "status": o.status.tag(), "subject": o.subject, "detail": o.detail }))
        .collect()
}

pub(super) fn list_blocking(env: &McpEnv, req: SelectionArgs) -> Result<(serde_json::Value, bool), String> {
    let call = env.resolve_call(req.profile.as_deref(), req.package.as_deref())?;

    let mut selection = to_selection(&req);
    if req.last_failed.unwrap_or(false) {
        if let Some(paths) = crate::load_last_failed(&Some(call.home.clone())) {
            selection.nodes.extend(paths);
        }
    }

    let suites = crate::collect_suites(&call.base_dir, &call.declared, &call.proofs, true)?;
    let mut config = crate::engine_config(1, &call.dependencies, Some(&call.home), prova_core::progress::null())
        .with_capabilities(call.capabilities.clone())
        .with_promises_only(req.promises.unwrap_or(false));
    config.selection = selection;

    let mut nodes: Vec<serde_json::Value> = Vec::new();
    for file in suites.iter().flat_map(|s| &s.files) {
        let node_paths =
            discover_path_with(file, &config).map_err(|e| format!("{}: {e}", file.display()))?;
        nodes.extend(node_paths.into_iter().map(|p| json!({ "path": p })));
    }
    Ok((json!({ "nodes": nodes }), false))
}

pub(super) fn eval_blocking(
    env: &McpEnv,
    code: String,
    package: Option<String>,
) -> Result<(serde_json::Value, bool), String> {
    if code.trim().is_empty() {
        return Err("eval: the snippet is empty".into());
    }
    // `eval` deliberately works with NO manifest (the built-ins alone are useful), so it cannot go
    // through `resolve_call`, which requires one. A `package` still targets another suite: resolve
    // its home + plugins so `require(...)` and `prova.root` mean what they mean *there*.
    let (home, plugins) = match package.as_deref() {
        None => (env.home.clone(), env.dependencies.clone()),
        Some(p) => {
            let call = env.resolve_call(None, Some(p))?;
            (Some(call.home), call.dependencies)
        }
    };
    let config = crate::engine_config(1, &plugins, home.as_ref(), prova_core::progress::null());
    eval_snippet(&code, &config)
        .map(|value| (value, false))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Warm tool bodies (each runs under `blocking`, talking to a holder thread where needed)
// ---------------------------------------------------------------------------------------------

pub(super) fn up_blocking(
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

    let call = env.resolve_call(req.profile.as_deref(), req.package.as_deref())?;

    // The registration door, and no other (the same rule as CLI `up` — `build_topology_run` in
    // main.rs): the inhabited verbs stand up `[topologies]` registrations ONLY, never a topology
    // scanned out of test files. A test-local declaration is a fixture, not an environment.
    if call.topologies.is_empty() {
        return Err(format!(
            "up {name:?}: no topologies registered — add it to [topologies] in prova.toml, e.g.\n  \
             [topologies]\n  {name} = {{ package = \"<package>\", topology = \"<advertised>\" }}"
        ));
    }
    if !call.topologies.contains_key(&name) {
        let known: Vec<&str> = call.topologies.keys().map(String::as_str).collect();
        return Err(format!(
            "up {name:?}: not in [topologies] (registered: {})",
            known.join(", ")
        ));
    }

    let mut config = crate::engine_config(1, &call.dependencies, Some(&call.home), prova_core::progress::null())
        .with_capabilities(call.capabilities.clone())
        .with_ports(if req.fixed.unwrap_or(false) {
            PortMode::Fixed
        } else {
            PortMode::Auto
        });
    let mut requested_requires: Vec<String> = Vec::new();
    for (alias, decl) in &call.topologies {
        let resolved = packages::resolve_topology(alias, decl, &call.dependencies)
            .map_err(|e| format!("up {name:?}: {e}"))?;
        let options = crate::topology_options_to_lua(&decl.options);
        config = config.with_topology_registration(alias, &decl.package, resolved.factory, options);
        if alias == &name {
            requested_requires = resolved.requires;
        }
    }
    // The universal `requires` gate: every registration carries its advertisement's environment
    // requirements, checked before anything is provisioned.
    for req_expr in &requested_requires {
        match call.capabilities.expr_status(req_expr) {
            Ok(None) => {}
            Ok(Some(reason)) => {
                return Err(format!(
                    "up {name:?}: cannot stand up: it requires {reason}"
                ));
            }
            Err(e) => {
                return Err(format!(
                    "up {name:?}: invalid requires {req_expr:?}: {e}"
                ));
            }
        }
    }

    // Warm runs re-read the package's test files; provisioning itself loads none.
    let files = package_test_files(&call)?;

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
                    home: call.home.clone(),
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

pub(super) fn down_blocking(warm: &WarmRegistry, name: &str) -> Result<(serde_json::Value, bool), String> {
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
pub(super) fn warm_run_blocking(
    _env: &McpEnv,
    warm: &WarmRegistry,
    topology: &str,
    req: RunRequest,
) -> Result<(serde_json::Value, bool), String> {
    let (tx, home) = warm
        .lock()
        .expect("warm registry")
        .get(topology)
        .map(|h| (h.tx.clone(), h.home.clone()))
        .ok_or_else(|| not_held(topology))?;

    // The warm holder's engine config is fixed at `up`; per-run spec modes would silently not
    // apply. The burndown loop is a cold loop anyway (implement → recompile → re-run).
    if req.selection.promises.unwrap_or(false) || req.due.unwrap_or(false) {
        return Err(
            "promises/due are not supported on warm runs — omit `topology` to run the \
             spec burndown cold"
                .to_string(),
        );
    }

    let mut selection = to_selection(&req.selection);
    // `last_failed` state lives in the held topology's home — the package its `up` resolved —
    // so warm and cold runs on that package share one red set.
    let lf_home = Some(home);
    let mut note: Option<String> = None;
    if req.selection.last_failed.unwrap_or(false) {
        match crate::load_last_failed(&lf_home) {
            Some(paths) if !paths.is_empty() => selection.nodes.extend(paths),
            _ => {
                note = Some(
                    "last_failed: no failure state from a previous run here; ran everything"
                        .to_string(),
                )
            }
        }
    }

    let had_selection = !selection.is_empty();
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    tx.send(WarmCmd::Run {
        selection,
        reply: reply_tx,
    })
    .map_err(|_| not_held(topology))?;
    let outcome = reply_rx.recv().map_err(|_| not_held(topology))??;

    // The CLI's empty-selection contract, mirrored (see `run_blocking`). No spec term here: warm
    // runs refuse promises/due outright, so a promised count cannot arise.
    if outcome.passed + outcome.failed + outcome.skipped == 0 && had_selection {
        return Err(format!(
            "selection matched no tests ({} deselected) — usually a typo; loosen the selection \
             or check `list`",
            outcome.deselected
        ));
    }

    // Keep the `--last-failed` state in step with cold runs — every transport shares one loop.
    let failed_paths: Vec<String> = outcome.failures.iter().map(|f| f.path.clone()).collect();
    crate::store_last_failed(&lf_home, &failed_paths);

    let failures: Vec<serde_json::Value> = outcome.failures.iter().map(failure_json).collect();
    let mut result = json!({
        "passed": outcome.passed,
        "failed": outcome.failed,
        "skipped": outcome.skipped,
        "deselected": outcome.deselected,
        "duration_ms": outcome.duration_ms,
        "failures": failures,
        "topology": topology,
    });
    if let Some(n) = note {
        result["note"] = json!(n);
    }
    Ok((result, outcome.failed > 0))
}

pub(super) fn warm_eval_blocking(
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
