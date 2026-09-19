//! The run journal and `--resume`, planned from the invocation (docs/plans/resume.md).
//!
//! Reading and writing the journal belong to `crate::journal`. This is the half that needs the flag
//! struct: what key a run is filed under, whether it keeps a journal at all, what a resume may carry
//! forward, and what the run record takes from the journal at the end.

use super::*;

use std::collections::BTreeMap;

/// What this run journals, and what `--resume` lets it carry forward.
pub(super) struct JournalPlan {
    header: journal::Header,
    /// The offered passes: file-qualified path → the run that executed it. Empty without `--resume`.
    reuse: BTreeMap<String, String>,
}

/// Narrowed by the CLI, as opposed to the lane's own baked tags, which ARE the lane.
fn cli_narrowed(cli: &Cli, config: &prova_core::RunConfig) -> bool {
    let s = &config.selection;
    !(s.keywords.is_empty()
        && s.keyword_excludes.is_empty()
        && s.tags.is_empty()
        && s.tag_excludes.is_empty()
        && s.nodes.is_empty()
        && s.covering.is_empty())
        || config.promises_only
        || config.proofs_only
        || config.falsify
        || !cli.explicit_paths.is_empty()
}

/// The invocation key a journal is filed under and a resume must match: the lane and its baked
/// tags, the manifest, the thrown switches, ad-hoc packages, and the flags that change a verdict.
fn journal_key(cli: &Cli, config: &prova_core::RunConfig) -> Vec<String> {
    let mut key = vec![format!("profile={}", cli.profile.as_deref().unwrap_or("default"))];
    if let Some(m) = &cli.manifest_path {
        key.push(format!("manifest={m}"));
    }
    key.extend(config.selection.lane_tags.iter().map(|t| format!("lane-tag={t}")));
    key.extend(config.selection.lane_tag_excludes.iter().map(|t| format!("lane-tag=!{t}")));
    key.extend(config.switches.iter().map(|s| format!("switch={s}")));
    key.extend(cli.packages.iter().map(|p| format!("package={p}")));
    if config.due {
        key.push("--due".into());
    }
    if cli.update_snapshots {
        key.push("--update-snapshots".into());
    }
    key
}

/// A `--resume` refusal: said once, exit 2.
fn refuse<T>(why: impl std::fmt::Display) -> Result<T, ExitCode> {
    eprintln!("prova: --resume: {why}");
    Err(ExitCode::from(2))
}

/// Plan the journal every UNNARROWED run keeps and, under `--resume`, hand the engine the passes it
/// may carry forward (`config.reuse`). A resume that cannot be honoured REFUSES (exit 2), naming
/// why: one that silently ran the whole lane would hold its caller for the full suite while it
/// believed it was resuming.
pub(super) fn plan_journal(
    cli: &Cli,
    home: &Option<Home>,
    config: &mut prova_core::RunConfig,
) -> Result<Option<JournalPlan>, ExitCode> {
    if cli.list || cli.switches_list || cli.reminders_list || cli.backfill {
        return Ok(None);
    }
    if cli_narrowed(cli, config) {
        if cli.resume {
            return refuse(
                "a resume carries a whole LANE forward, and -k/--tags/--node/--covering/\
                 --last-failed/--falsify/--promises/--proofs or explicit paths narrow it — drop \
                 them, or use --last-failed for an inner loop that attests nothing",
            );
        }
        return Ok(None);
    }
    if cli.resume && cli.update_baseline.is_some() {
        return refuse(
            "--update-baseline banks this run's measurements, and a reused test took none — bank \
             from a run without --resume",
        );
    }
    let Some(h) = home.as_ref() else {
        if cli.resume {
            return refuse("there is no package home, so no journal to resume from");
        }
        return Ok(None);
    };
    let tree = match crate::tree::fingerprint(&h.dir) {
        Ok(t) => t,
        Err(why) if cli.resume => return refuse(why),
        Err(_) => return Ok(None),
    };
    let key = journal_key(cli, config);
    let binary = record::binary_fingerprint();
    let reuse = if cli.resume { carried(h, &key, &tree, &binary)? } else { BTreeMap::new() };
    *config = std::mem::take(config).with_reuse(reuse.keys().cloned());
    Ok(Some(JournalPlan {
        header: journal::Header {
            schema: 1,
            run_id: journal::new_run_id(),
            key,
            tree,
            binary,
            started_at: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
        },
        reuse,
    }))
}

/// The passes the previous run of this key lends a resume, or the refusal that says why it lends
/// none: nothing journaled, the tree changed since or during it, or prova itself changed.
fn carried(
    home: &Home,
    key: &[String],
    tree: &str,
    binary: &str,
) -> Result<BTreeMap<String, String>, ExitCode> {
    let Some(prior) = journal::load(home, key) else {
        return refuse(format!(
            "no earlier run of this lane ({}) is journaled here — run it once without --resume",
            key.join(" ")
        ));
    };
    let who = format!("run {} (started {})", prior.header.run_id, prior.header.started_at);
    if prior.header.tree != tree {
        return refuse(format!(
            "the tracked tree changed since {who} — a verdict is reused only over the identical \
             bytes. Run the lane, or --last-failed for an inner loop that attests nothing"
        ));
    }
    if prior.end == Some(None) {
        return refuse(format!(
            "the tracked tree changed DURING {who}, so its verdicts belong to no single tree"
        ));
    }
    if prior.header.binary != binary {
        return refuse(format!(
            "prova itself changed since {who} (binary {} → {binary}) — a verdict is reused only \
             from the same prova",
            prior.header.binary
        ));
    }
    let reuse = prior.passes();
    let killed = if prior.end.is_none() { ", which never finished," } else { "" };
    eprintln!(
        "prova: --resume: {} pass{} carried forward from {who}{killed} over the same tree; \
         executing the rest",
        reuse.len(),
        if reuse.len() == 1 { "" } else { "es" }
    );
    Ok(reuse)
}

/// Open the journal a planned run keeps. The offered passes go in first, so a run killed before its
/// end still lends them to the next resume; a leaf that executes after all (an executing leaf
/// depended on it) is journaled again when it settles, and the later row is the one a reader
/// believes.
pub(super) fn open_journal(
    plan: Option<&JournalPlan>,
    home: &Option<Home>,
) -> Option<journal::Writer> {
    let (plan, h) = (plan?, home.as_ref()?);
    let mut writer = journal::Writer::create(h, &plan.header);
    for (path, from) in &plan.reuse {
        writer.row(journal::Row {
            path: path.clone(),
            outcome: "reused".into(),
            from: Some(from.clone()),
        });
    }
    Some(writer)
}

/// What the run record takes from the journal once the run is over.
pub(super) struct Settled {
    pub(super) run_id: String,
    pub(super) tree: Option<String>,
    pub(super) executed: BTreeMap<String, record::Executed>,
    pub(super) reused_from: BTreeMap<String, String>,
}

/// Close the journal and settle the record's account of it.
///
/// The tree is confirmed at the END as well: a record whose tree moved during the run belongs to no
/// single tree, so it carries none, and no resume will ever lend its passes. What the engine
/// actually held back is spelled `reused`, never `passed`.
pub(super) fn settle(
    plan: Option<&JournalPlan>,
    home: &Option<Home>,
    reporter: &mut FailureRecorder,
    summary: &prova_core::Summary,
) -> Settled {
    let tree = plan.and_then(|plan| {
        let after = home.as_ref().and_then(|h| crate::tree::fingerprint(&h.dir).ok());
        if after.as_deref() == Some(plan.header.tree.as_str()) {
            Some(plan.header.tree.clone())
        } else {
            eprintln!(
                "prova: the tracked tree changed during this run — its verdicts belong to no single \
                 tree, so it cannot be resumed from"
            );
            None
        }
    });
    if let Some(writer) = reporter.journal.as_mut() {
        writer.end(tree.clone());
    }
    let mut executed = std::mem::take(&mut reporter.executed);
    let mut reused_from = BTreeMap::new();
    for path in &summary.reused_paths {
        executed.insert(path.clone(), record::Executed::Reused);
        if let Some(from) = plan.and_then(|p| p.reuse.get(path)) {
            reused_from.insert(path.clone(), from.clone());
        }
    }
    Settled {
        run_id: plan.map(|p| p.header.run_id.clone()).unwrap_or_else(journal::new_run_id),
        tree,
        executed,
        reused_from,
    }
}
