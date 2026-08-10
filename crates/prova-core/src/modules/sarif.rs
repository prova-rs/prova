//! The `sarif` namespace — the findings-ingestion seam (docs/design/verifiers.md).
//!
//! SARIF (OASIS) is the de facto interchange for static-analysis findings — the format GitHub code
//! scanning consumes and clippy, ESLint, and dozens of linters emit. It is to findings what JUnit is
//! to test results, so one tolerant parser lets any of them flow into a proof's deputed account:
//! `sarif.load` turns result files into findings, `sarif.verify` (a Lua recipe) conducts a linter and
//! adopts its verdict, `sarif.ingest` files the findings into the run record.
//!
//! Native and bundled for the same reason junit is: first-party, consuming only SARIF's stable core
//! — tool name, ruleId, level, message, and the first physical location. Unknown fields are ignored.
//! A clean run (zero results) is a valid green — the inverse of junit, where zero cases means a wrong
//! glob.

use std::path::PathBuf;

use mlua::{Lua, Table};
use serde_json::Value;

use crate::model::{DeputedCase, DeputedRegistry};

/// One finding, pre-Lua. `level` keeps SARIF's own vocabulary.
struct Finding {
    tool: String,
    rule: String,
    level: String,
    message: String,
    uri: String,
    line: Option<u64>,
    /// The SARIF artifact it was read from — the provenance that makes adoption auditable.
    file: String,
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// Parse one SARIF document, appending its findings. Tolerant: fields the stable core does not need
/// are ignored, and a result with no `level` defaults to `"warning"` (SARIF's own default when a
/// rule's configuration is absent).
fn parse_document(json: &str, file: &str, findings: &mut Vec<Finding>) -> Result<(), String> {
    let doc: Value =
        serde_json::from_str(json).map_err(|e| format!("{file}: malformed SARIF JSON: {e}"))?;
    let Some(runs) = doc.get("runs").and_then(|r| r.as_array()) else {
        return Ok(());
    };
    for run in runs {
        let tool = run
            .get("tool")
            .and_then(|t| t.get("driver"))
            .and_then(|d| str_at(d, "name"))
            .unwrap_or_default()
            .to_string();
        let Some(results) = run.get("results").and_then(|r| r.as_array()) else {
            continue;
        };
        for res in results {
            let rule = str_at(res, "ruleId")
                .or_else(|| res.get("rule").and_then(|r| str_at(r, "id")))
                .unwrap_or_default()
                .to_string();
            let level = str_at(res, "level").unwrap_or("warning").to_string();
            let message = res
                .get("message")
                .and_then(|m| str_at(m, "text"))
                .unwrap_or_default()
                .to_string();
            // The first physical location, when present — where the finding sits in the source.
            let phys = res
                .get("locations")
                .and_then(|l| l.as_array())
                .and_then(|a| a.first())
                .and_then(|loc| loc.get("physicalLocation"));
            let uri = phys
                .and_then(|p| p.get("artifactLocation"))
                .and_then(|a| str_at(a, "uri"))
                .unwrap_or_default()
                .to_string();
            let line = phys
                .and_then(|p| p.get("region"))
                .and_then(|r| r.get("startLine"))
                .and_then(|n| n.as_u64());
            findings.push(Finding {
                tool: tool.clone(),
                rule,
                level,
                message,
                uri,
                line,
                file: file.to_string(),
            });
        }
    }
    Ok(())
}

/// Build the report table `sarif.load` returns: level counts, files (path + mtime), and findings.
fn report_table(lua: &Lua, files: &[PathBuf], findings: &[Finding]) -> mlua::Result<Table> {
    let report = lua.create_table()?;
    let count = |lvl: &str| findings.iter().filter(|f| f.level == lvl).count();
    report.set("total", findings.len())?;
    report.set("errors", count("error"))?;
    report.set("warnings", count("warning"))?;
    report.set("notes", count("note"))?;

    report.set("files", super::ingest::files_table(lua, files)?)?;

    let cases_t = lua.create_table()?;
    for (i, f) in findings.iter().enumerate() {
        let row = lua.create_table()?;
        row.set("tool", f.tool.as_str())?;
        row.set("rule", f.rule.as_str())?;
        row.set("level", f.level.as_str())?;
        row.set("message", f.message.as_str())?;
        row.set("uri", f.uri.as_str())?;
        if let Some(l) = f.line {
            row.set("line", l)?;
        }
        row.set("file", f.file.as_str())?;
        cases_t.set(i + 1, row)?;
    }
    report.set("cases", cases_t)?;
    Ok(report)
}

/// The `sarif.verify` facet — Lua over the primitives, the same shape junit.verify follows: run the
/// linter tolerantly (its exit code is not the verdict — its findings are), enforce freshness in run
/// mode, ingest, and fail on any finding at or above the threshold level (default "error"), tolerating
/// up to `opts.max`. Unlike junit there is NO vacuity guard: zero findings is a clean pass.
const VERIFY_RECIPE: &str = r##"
function sarif.verify(t, opts)
  opts = opts or {}
  if not opts.results then
    error("sarif.verify: results = <path or glob> is required — where the linter writes its SARIF")
  end
  local rank = { none = 0, note = 1, warning = 2, error = 3 }
  local started = os.time()
  if opts.run then
    shell.run(opts.run, { cwd = opts.cwd, merge_stderr = true })
  end
  local report = sarif.load(opts.results, { cwd = opts.cwd })
  if opts.run then
    if #report.files == 0 then
      error("sarif.verify: the run produced no SARIF at " .. opts.results)
    end
    for _, f in ipairs(report.files) do
      if f.mtime + 1 < started then
        error("sarif.verify: " .. f.path .. " predates the run command — a stale artifact is not this run's evidence")
      end
    end
  end
  sarif.ingest(report)
  local threshold = rank[opts.level or "error"] or 3
  local red = {}
  for _, c in ipairs(report.cases) do
    if (rank[c.level] or 2) >= threshold then
      local where = c.uri or ""
      if c.line then where = where .. ":" .. c.line end
      red[#red + 1] = c.rule .. " " .. where .. " — " .. (c.message or c.level)
    end
  end
  t:expect(#red, "sarif findings at or above '" .. (opts.level or "error") .. "':\n  "
    .. table.concat(red, "\n  ")):never():gt(opts.max or 0)
  return report
end
"##;

pub(crate) fn make(lua: &Lua, deputed: Option<DeputedRegistry>) -> mlua::Result<Table> {
    let sarif = lua.create_table()?;

    sarif.set(
        "load",
        lua.create_function(|lua, (pattern, opts): (String, Option<Table>)| {
            let cwd: Option<String> = match &opts {
                Some(o) => o.get("cwd")?,
                None => None,
            };
            let files =
                super::ingest::resolve_files(&pattern, cwd.as_deref(), "sarif.load")
                    .map_err(mlua::Error::RuntimeError)?;
            let mut findings = Vec::new();
            for f in &files {
                let json = std::fs::read_to_string(f).map_err(|e| {
                    mlua::Error::RuntimeError(format!("sarif.load: cannot read {}: {e}", f.display()))
                })?;
                parse_document(&json, &f.to_string_lossy(), &mut findings)
                    .map_err(mlua::Error::RuntimeError)?;
            }
            report_table(lua, &files, &findings)
        })?,
    )?;

    // ingest(report) — file the report's findings into the run's deputed account (verifier "sarif").
    // A no-op when no registry is attached, so `sarif.load` in an `eval` pollutes nothing.
    sarif.set(
        "ingest",
        lua.create_function(move |_, report: Table| {
            let Some(registry) = deputed.as_ref() else {
                return Ok(0usize);
            };
            let cases: Table = report.get("cases")?;
            let mut rows = Vec::new();
            for entry in cases.sequence_values::<Table>() {
                let c = entry?;
                let rule: String = c.get::<Option<String>>("rule")?.unwrap_or_default();
                let uri: String = c.get::<Option<String>>("uri")?.unwrap_or_default();
                let line: Option<u64> = c.get("line")?;
                // The finding's source location rides the name; DeputedCase.file is the SARIF file.
                let name = if uri.is_empty() {
                    rule
                } else if let Some(l) = line {
                    format!("{rule} @ {uri}:{l}")
                } else {
                    format!("{rule} @ {uri}")
                };
                rows.push(DeputedCase {
                    verifier: "sarif".to_string(),
                    suite: c.get::<Option<String>>("tool")?.unwrap_or_default(),
                    name,
                    outcome: c.get::<Option<String>>("level")?.unwrap_or_default(),
                    message: c.get("message")?,
                    time_ms: None,
                    file: c.get::<Option<String>>("file")?.unwrap_or_default(),
                });
            }
            let n = rows.len();
            // Recover a poisoned lock: the account is a plain Vec, valid at every step.
            registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(rows);
            Ok(n)
        })?,
    )?;

    Ok(sarif)
}

/// Load the `sarif.verify` recipe — after `make`'s table is installed as the global.
pub(crate) fn install_recipe(lua: &Lua) -> mlua::Result<()> {
    lua.load(VERIFY_RECIPE).set_name("@prova/sarif").exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIPPY_SARIF: &str = r#"{
      "version": "2.1.0",
      "runs": [{
        "tool": { "driver": { "name": "clippy" } },
        "results": [
          {
            "ruleId": "clippy::unwrap_used",
            "level": "warning",
            "message": { "text": "used `unwrap()` on a `Result` value" },
            "locations": [{ "physicalLocation": {
              "artifactLocation": { "uri": "src/main.rs" },
              "region": { "startLine": 42 }
            }}]
          },
          {
            "ruleId": "clippy::correctness",
            "level": "error",
            "message": { "text": "this will panic at runtime" },
            "locations": [{ "physicalLocation": {
              "artifactLocation": { "uri": "src/lib.rs" },
              "region": { "startLine": 7 }
            }}]
          }
        ]
      }]
    }"#;

    #[test]
    fn parses_the_stable_core_of_a_clippy_document() {
        let mut findings = Vec::new();
        parse_document(CLIPPY_SARIF, "r.sarif", &mut findings).unwrap();
        assert_eq!(findings.len(), 2);
        assert_eq!(
            (
                findings[0].tool.as_str(),
                findings[0].rule.as_str(),
                findings[0].level.as_str()
            ),
            ("clippy", "clippy::unwrap_used", "warning")
        );
        assert_eq!(findings[0].uri, "src/main.rs");
        assert_eq!(findings[0].line, Some(42));
        assert_eq!(findings[1].level, "error");
    }

    #[test]
    fn a_clean_run_is_empty_not_an_error() {
        let clean = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"clippy"}},"results":[]}]}"#;
        let mut findings = Vec::new();
        parse_document(clean, "clean.sarif", &mut findings).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn malformed_json_is_a_loud_error_never_an_empty_green() {
        let mut findings = Vec::new();
        let err = parse_document("{not json", "bad.sarif", &mut findings).unwrap_err();
        assert!(err.contains("bad.sarif"), "{err}");
    }
}
