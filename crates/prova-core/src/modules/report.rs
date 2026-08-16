//! `report.publish` — a conduct hands back the artifact it produced, and prova takes custody.
//!
//! The seam this closes: a deputed conduct produces three things, and prova adopted two of them.
//! Cases go to the ledger (`attest`, `evidence`), measurements go to the ratchets, and the deputy's
//! own report — llvm-cov's HTML, its per-file JSON, a junit file — was dropped. It landed under
//! `target/`, which `xtask sweep` deletes, and nothing named it. So the coverage floor could refuse
//! a regression and be unable to show which lines moved, having had that answer in hand.
//!
//! **Custody, not visualization** (docs/design/verifiers.md#reports-are-custody-not-visualization).
//! Prova renders exactly what it rendered before — counts and rows, here the one-line `summary`. The
//! deputy rendered the artifact; prova preserves it, names it, and makes it addressable. That
//! boundary is what keeps this from becoming the dashboard the design says it is not.

use super::*;
use crate::model::{Report, ReportForm, ReportRegistry};

/// Copy a published artifact under the run's custody root, returning where it now lives.
///
/// Copied rather than referenced, and copied NOW rather than at drain time. `target/` is swept and
/// a fixture's `tempdir` is reaped at scope end, so a path recorded and left alone is a link that
/// rots — and a report that rots is worse than no report, because it reads as available. Publishing
/// is the moment the file is certain to exist.
///
/// A directory (llvm-cov's HTML tree is one) is copied whole; anything unreadable is left where it
/// is and reported at its original path rather than failing the run — a proof does not deserve to
/// go red because its evidence could not be filed.
fn take_custody(root: &std::path::Path, name: &str, form: &ReportForm) -> std::path::PathBuf {
    let leaf = form
        .path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| std::ffi::OsString::from(&form.kind));
    let dest_dir = root.join(name);
    if std::fs::create_dir_all(&dest_dir).is_err() {
        return form.path.clone();
    }
    let dest = dest_dir.join(&leaf);
    if form.path.is_dir() {
        if copy_tree(&form.path, &dest).is_err() {
            return form.path.clone();
        }
        // A tree is preserved WHOLE — llvm-cov's HTML links to a page per file, and copying only
        // the entry point would file a report whose every link is broken. But the address a reader
        // wants is the door, not the building: recording `index.html` when the tree has one is what
        // makes `open $(prova reports coverage --kind html)` land on the report itself. Addressing
        // an entry point, not interpreting content — the boundary still holds.
        let index = dest.join("index.html");
        return if index.is_file() { index } else { dest };
    }
    if std::fs::copy(&form.path, &dest).is_ok() {
        dest
    } else {
        form.path.clone()
    }
}

/// Recursive directory copy — llvm-cov's HTML report is a tree of pages, and half of one is not a
/// report. `std::fs` has no tree copy, and pulling a crate in for eight lines is not worth it.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Read the `forms` list: `{ { kind = "json", path = "..." }, ... }`, or the shorthand
/// `{ json = "...", html = "..." }` — the map spelling reads better when each kind appears once,
/// which is every case so far.
fn parse_forms(table: &Table) -> mlua::Result<Vec<ReportForm>> {
    let mut forms = Vec::new();
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair?;
        match (&key, &value) {
            // The list spelling: an array entry carrying its own `kind` and `path`.
            (Value::Integer(_), Value::Table(entry)) => {
                let kind: String = entry.get("kind")?;
                let path: String = entry.get("path")?;
                forms.push(ReportForm { kind, path: path.into() });
            }
            // The map spelling: `json = "path"`.
            (Value::String(kind), Value::String(path)) => forms.push(ReportForm {
                kind: kind.to_string_lossy().to_string(),
                path: path.to_string_lossy().to_string().into(),
            }),
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "report.publish: `forms` takes { kind = path, … } or { { kind = …, path = … }, … }"
                        .to_string(),
                ))
            }
        }
    }
    forms.sort_by(|a, b| a.kind.cmp(&b.kind));
    Ok(forms)
}

/// Build the `report` global. `custody_root` is `<home>/.prova/var/reports` when a run has a
/// package; without one (a bare `prova file.lua`) the artifact is recorded where it already lives,
/// which is honest — there is nowhere durable to put it.
pub(crate) fn make(
    lua: &Lua,
    reports: Option<ReportRegistry>,
    custody_root: Option<std::path::PathBuf>,
) -> mlua::Result<Table> {
    let report = lua.create_table()?;

    // publish{ name, summary, forms, explains? } — file an artifact into the run's report account.
    // A no-op when no registry is attached (an `eval`, a bare embedder), matching `measure.record`,
    // so publishing never leaks outside a run.
    report.set(
        "publish",
        lua.create_function(move |_, opts: Table| {
            crate::opts::reject_unknown(&opts, &["name", "summary", "forms", "explains"], "report.publish")?;
            let name: String = opts.get("name")?;
            let summary: String = opts.get("summary")?;
            if name.trim().is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "report.publish: `name` is the address — it cannot be empty".to_string(),
                ));
            }
            // A summary is REQUIRED. A report whose gist you cannot read without opening it is a
            // file path with extra steps, and the one-liner is the only part prova itself renders.
            if summary.trim().is_empty() {
                return Err(mlua::Error::RuntimeError(format!(
                    "report.publish {name:?}: `summary` is required — one line saying what the \
                     artifact shows, so a reader (or an agent) needs no viewer to get the gist"
                )));
            }
            let forms = match opts.get::<Option<Table>>("forms")? {
                Some(t) => parse_forms(&t)?,
                None => Vec::new(),
            };
            if forms.is_empty() {
                return Err(mlua::Error::RuntimeError(format!(
                    "report.publish {name:?}: no `forms` — a report with no artifact is a log line"
                )));
            }
            let explains = match opts.get::<Option<Value>>("explains")? {
                Some(Value::String(s)) => vec![s.to_string_lossy().to_string()],
                Some(Value::Table(t)) => t
                    .sequence_values::<String>()
                    .collect::<mlua::Result<Vec<_>>>()?,
                _ => Vec::new(),
            };

            // Custody first, so the record names where the artifact will still be.
            let forms = match custody_root.as_ref() {
                Some(root) => forms
                    .into_iter()
                    .map(|f| {
                        let path = take_custody(root, &name, &f);
                        ReportForm { kind: f.kind, path }
                    })
                    .collect(),
                None => forms,
            };

            if let Some(registry) = reports.as_ref() {
                let mut held = registry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Re-publishing a name REPLACES it: a conduct that runs twice in one run (a
                // re-measure, a retried fixture) should leave one current report, not two rows
                // disagreeing about which is the answer.
                held.retain(|r: &Report| r.name != name);
                held.push(Report { name, summary, explains, forms });
            }
            Ok(())
        })?,
    )?;

    Ok(report)
}

#[cfg(test)]
#[path = "report/report_tests.rs"]
mod report_tests;
