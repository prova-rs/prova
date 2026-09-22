//! The `junit` namespace — the verdict-ingestion seam (docs/design/verifiers.md).
//!
//! JUnit XML is the de facto lingua franca of test results, which is what makes one tolerant
//! parser the highest-leverage integration prova can carry: `junit.load` turns result files into
//! named cases, `junit.verify` (a Lua recipe over it) conducts a deputy and adopts its verdict,
//! and `junit.ingest` files the cases into the run's deputed account for the record.
//!
//! Bundled and native, not a package: XML parsing cannot be written in Lua, and native code is
//! always first-party. The parser consumes only the format's stable core — suite/case names,
//! outcome, message, timing — and tolerates dialect drift (unknown elements and attributes are
//! ignored, nested `<testsuite>` groups are walked).

use std::path::PathBuf;

use mlua::{Lua, Table};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::model::{DeputedCase, DeputedRegistry};

/// One parsed case, pre-Lua. `outcome` keeps JUnit's own vocabulary.
struct Case {
    suite: String,
    name: String,
    outcome: &'static str,
    message: Option<String>,
    time_ms: Option<u64>,
    file: String,
    /// The run that produced this verdict, when it was carried forward rather than conducted here:
    /// the `prova.reused_from` property `junit.merge` writes (docs/plans/resume.md#phase-1b).
    reused_from: Option<String>,
}

/// The testcase property that marks a verdict carried forward by `junit.merge`.
const REUSED_FROM: &str = "prova.reused_from";

/// Parse one JUnit XML document, appending its cases. Tolerant by design: elements the stable
/// core doesn't include are skipped, a `<testcase>` with no failure/error/skipped child passed.
fn parse_document(xml: &str, file: &str, cases: &mut Vec<Case>) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    // The suite name in scope, from enclosing <testsuite name="..."> elements (they nest).
    let mut suite_stack: Vec<String> = Vec::new();
    // The case currently open, when <testcase> was not self-closing.
    let mut open: Option<Case> = None;

    let attr = |e: &quick_xml::events::BytesStart, name: &str| -> Option<String> {
        e.attributes().flatten().find_map(|a| {
            (a.key.as_ref() == name.as_bytes()).then(|| {
                // Unescape entities — `&amp;` in a message must reach the report as `&`.
                a.unescape_value()
                    .map(|v| v.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned())
            })
        })
    };
    let start_case = |e: &quick_xml::events::BytesStart, suite_stack: &[String]| -> Case {
        // `classname` is the case's own suite when present (the common dialect); the enclosing
        // <testsuite name> is the fallback.
        let suite = attr(e, "classname")
            .or_else(|| suite_stack.last().cloned())
            .unwrap_or_default();
        let time_ms = attr(e, "time")
            .and_then(|t| t.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0) as u64);
        Case {
            suite,
            name: attr(e, "name").unwrap_or_default(),
            outcome: "passed",
            message: None,
            time_ms,
            file: file.to_string(),
            reused_from: None,
        }
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"testsuite" => suite_stack.push(attr(&e, "name").unwrap_or_default()),
                b"testcase" => open = Some(start_case(&e, &suite_stack)),
                b"failure" | b"error" | b"skipped" => {
                    if let Some(case) = open.as_mut() {
                        case.outcome = match e.name().as_ref() {
                            b"failure" => "failed",
                            b"error" => "error",
                            _ => "skipped",
                        };
                        case.message = attr(&e, "message");
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"testcase" => cases.push(start_case(&e, &suite_stack)),
                // Only a case's own property: a suite-level <properties> block has no open case.
                b"property" if attr(&e, "name").as_deref() == Some(REUSED_FROM) => {
                    if let Some(case) = open.as_mut() {
                        case.reused_from = attr(&e, "value");
                    }
                }
                b"failure" | b"error" | b"skipped" => {
                    if let Some(case) = open.as_mut() {
                        case.outcome = match e.name().as_ref() {
                            b"failure" => "failed",
                            b"error" => "error",
                            _ => "skipped",
                        };
                        case.message = attr(&e, "message");
                    }
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                // A failure/error body with no `message` attribute carries its text as content —
                // the pytest dialect. First line only: the message is a label, not a traceback.
                if let Some(case) = open.as_mut() {
                    if case.outcome != "passed" && case.message.is_none() {
                        let text = t.unescape().unwrap_or_default();
                        let first = text.lines().next().unwrap_or("").trim().to_string();
                        if !first.is_empty() {
                            case.message = Some(first);
                        }
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"testsuite" => {
                    suite_stack.pop();
                }
                b"testcase" => {
                    if let Some(case) = open.take() {
                        cases.push(case);
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("{file}: malformed XML: {e}")),
            Ok(_) => {}
        }
    }
    Ok(())
}

/// Build the report table `junit.load` returns: counts, files (path + mtime), and cases.
fn report_table(lua: &Lua, files: &[PathBuf], cases: &[Case]) -> mlua::Result<Table> {
    let report = lua.create_table()?;
    let count = |o: &str| cases.iter().filter(|c| c.outcome == o).count();
    report.set("total", cases.len())?;
    report.set("passed", count("passed"))?;
    report.set("failed", count("failed"))?;
    report.set("errors", count("error"))?;
    report.set("skipped", count("skipped"))?;

    report.set("files", super::ingest::files_table(lua, files)?)?;

    let cases_t = lua.create_table()?;
    for (i, c) in cases.iter().enumerate() {
        let row = lua.create_table()?;
        row.set("suite", c.suite.as_str())?;
        row.set("name", c.name.as_str())?;
        row.set("outcome", c.outcome)?;
        if let Some(m) = &c.message {
            row.set("message", m.as_str())?;
        }
        if let Some(t) = c.time_ms {
            row.set("time_ms", t)?;
        }
        row.set("file", c.file.as_str())?;
        if let Some(from) = &c.reused_from {
            row.set("reused_from", from.as_str())?;
        }
        cases_t.set(i + 1, row)?;
    }
    report.set("cases", cases_t)?;
    Ok(report)
}

/// The `junit.verify` facet — Lua over the primitives, exactly the shape verifier packages
/// follow. Kept as a recipe so the contract is readable at `prova learn`-level: run the deputy
/// tolerantly (its exit code is not the verdict — its results are), enforce freshness in run
/// mode, ingest, refuse vacuity, and fail with the deputed cases' own names.
const VERIFY_RECIPE: &str = r##"
function junit.verify(t, opts)
  opts = opts or {}
  if not opts.results then
    error("junit.verify: results = <path or glob> is required — where the deputy writes its XML")
  end
  local started = os.time()
  if opts.run then
    -- Tolerant on purpose: mvn/gradle/pytest exit non-zero on a failing test, and the structured
    -- report is what we are here for. The verdict is judged from the cases, loudly, below.
    shell.run(opts.run, { cwd = opts.cwd, merge_stderr = true })
  end
  local report = junit.load(opts.results, { cwd = opts.cwd })
  if opts.run then
    if #report.files == 0 then
      error("junit.verify: the run produced no result files at " .. opts.results)
    end
    for _, f in ipairs(report.files) do
      if f.mtime + 1 < started then
        error("junit.verify: " .. f.path .. " predates the run command — a stale artifact is not this run's evidence")
      end
    end
  end
  junit.ingest(report)
  t:expect(report.total, "junit.verify: zero cases parsed from " .. tostring(opts.results)
    .. " — wrong glob, or the deputy ran nothing"):gt(0)
  local red = {}
  for _, c in ipairs(report.cases) do
    if c.outcome == "failed" or c.outcome == "error" then
      red[#red + 1] = c.suite .. "#" .. c.name .. " — " .. (c.message or c.outcome)
    end
  end
  t:expect(#red, "deputed failures:\n  " .. table.concat(red, "\n  ")):equals(0)
  return report
end
"##;

pub(crate) fn make(lua: &Lua, deputed: Option<DeputedRegistry>) -> mlua::Result<Table> {
    let junit = lua.create_table()?;

    junit.set(
        "load",
        lua.create_function(|lua, (pattern, opts): (String, Option<Table>)| {
            let cwd: Option<String> = match &opts {
                Some(o) => o.get("cwd")?,
                None => None,
            };
            let files = super::ingest::resolve_files(&pattern, cwd.as_deref(), "junit.load")
                .map_err(mlua::Error::RuntimeError)?;
            let mut cases = Vec::new();
            for f in &files {
                let xml = std::fs::read_to_string(f).map_err(|e| {
                    mlua::Error::RuntimeError(format!("junit.load: cannot read {}: {e}", f.display()))
                })?;
                parse_document(&xml, &f.to_string_lossy(), &mut cases)
                    .map_err(mlua::Error::RuntimeError)?;
            }
            report_table(lua, &files, &cases)
        })?,
    )?;

    // ingest(report) — file the report's cases into the run's deputed account. What `verify`
    // calls after loading; a no-op when no registry is attached (an `eval`, a bare embedder), so
    // querying with `junit.load` never pollutes anything.
    junit.set(
        "ingest",
        lua.create_function(move |_, report: Table| {
            let Some(registry) = deputed.as_ref() else {
                return Ok(0usize);
            };
            let cases: Table = report.get("cases")?;
            let mut rows = Vec::new();
            for entry in cases.sequence_values::<Table>() {
                let c = entry?;
                rows.push(DeputedCase {
                    verifier: "junit".to_string(),
                    suite: c.get::<Option<String>>("suite")?.unwrap_or_default(),
                    name: c.get::<Option<String>>("name")?.unwrap_or_default(),
                    outcome: c.get::<Option<String>>("outcome")?.unwrap_or_default(),
                    message: c.get("message")?,
                    time_ms: c.get("time_ms")?,
                    file: c.get::<Option<String>>("file")?.unwrap_or_default(),
                    reused_from: c.get("reused_from")?,
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

    // merge(prior, rerun?, { reused_from, out }) — a deputy's full account after a resumed run
    // re-ran only its failures (docs/plans/resume.md#phase-1b). Every case of `prior` the rerun
    // did not re-run is carried forward and MARKED with the run it came from; a re-run case
    // replaces its prior verdict; the result is written to `out` and its path returned, so readers
    // that parse the artifact see the whole account and can tell carried from conducted.
    junit.set(
        "merge",
        lua.create_function(|_, (prior, rerun, opts): (String, Option<String>, Table)| {
            let reused_from: String = opts.get::<Option<String>>("reused_from")?.ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "junit.merge: reused_from = <the run the prior account came from> is required"
                        .into(),
                )
            })?;
            let out: String = opts.get::<Option<String>>("out")?.ok_or_else(|| {
                mlua::Error::RuntimeError("junit.merge: out = <path to write> is required".into())
            })?;
            let load = |path: &str| -> mlua::Result<Vec<Case>> {
                let xml = std::fs::read_to_string(path).map_err(|e| {
                    mlua::Error::RuntimeError(format!("junit.merge: cannot read {path}: {e}"))
                })?;
                let mut cases = Vec::new();
                parse_document(&xml, path, &mut cases).map_err(mlua::Error::RuntimeError)?;
                Ok(cases)
            };
            let prior_cases = load(&prior)?;
            let rerun_cases = match &rerun {
                Some(p) => load(p)?,
                None => Vec::new(),
            };
            let xml = merge_accounts(prior_cases, rerun_cases, &reused_from);
            std::fs::write(&out, xml).map_err(|e| {
                mlua::Error::RuntimeError(format!("junit.merge: cannot write {out}: {e}"))
            })?;
            Ok(out)
        })?,
    )?;

    Ok(junit)
}

/// Merge a prior account with the cases re-run over it. The prior's order is kept; a re-run case
/// replaces its (suite, name) twin; a re-run case the prior never had is appended. Every carried
/// case keeps the origin it already had (a resume of a resume names the run that CONDUCTED it) or
/// takes `reused_from`. Carried failures stay failures: a merge carries verdicts forward, and only
/// passes are evidence — a red that nobody re-ran is still red.
fn merge_accounts(prior: Vec<Case>, rerun: Vec<Case>, reused_from: &str) -> String {
    let mut fresh: std::collections::HashMap<(String, String), Case> = rerun
        .into_iter()
        .map(|c| ((c.suite.clone(), c.name.clone()), c))
        .collect();
    let mut order: Vec<Case> = Vec::new();
    for mut c in prior {
        match fresh.remove(&(c.suite.clone(), c.name.clone())) {
            Some(again) => order.push(again),
            None => {
                c.reused_from.get_or_insert_with(|| reused_from.to_string());
                order.push(c);
            }
        }
    }
    let mut left: Vec<Case> = fresh.into_values().collect();
    left.sort_by(|a, b| (&a.suite, &a.name).cmp(&(&b.suite, &b.name)));
    order.extend(left);
    render_account(&order)
}

/// Render cases as one JUnit document, grouped into suites in first-seen order.
fn render_account(cases: &[Case]) -> String {
    use quick_xml::escape::escape;
    let mut suites: Vec<(&str, Vec<&Case>)> = Vec::new();
    for c in cases {
        match suites.iter_mut().find(|(s, _)| *s == c.suite) {
            Some((_, v)) => v.push(c),
            None => suites.push((c.suite.as_str(), vec![c])),
        }
    }
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"prova-merged\">\n");
    for (suite, members) in suites {
        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\">\n",
            escape(suite),
            members.len()
        ));
        for c in members {
            let time = c.time_ms.map(|t| format!(" time=\"{:.3}\"", t as f64 / 1000.0)).unwrap_or_default();
            xml.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}\"{time}>\n",
                escape(&c.name),
                escape(&c.suite)
            ));
            let message = c.message.as_deref().map(|m| format!(" message=\"{}\"", escape(m))).unwrap_or_default();
            match c.outcome {
                "failed" => xml.push_str(&format!("      <failure{message}/>\n")),
                "error" => xml.push_str(&format!("      <error{message}/>\n")),
                "skipped" => xml.push_str("      <skipped/>\n"),
                _ => {}
            }
            if let Some(from) = &c.reused_from {
                xml.push_str(&format!(
                    "      <properties><property name=\"{REUSED_FROM}\" value=\"{}\"/></properties>\n",
                    escape(from)
                ));
            }
            xml.push_str("    </testcase>\n");
        }
        xml.push_str("  </testsuite>\n");
    }
    xml.push_str("</testsuites>\n");
    xml
}

/// Load the `junit.verify` recipe — after `make`'s table is installed as the global.
pub(crate) fn install_recipe(lua: &Lua) -> mlua::Result<()> {
    lua.load(VERIFY_RECIPE).set_name("@prova/junit").exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUREFIRE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.acme.OrderTest" tests="3" failures="1" errors="0" skipped="1" time="0.31">
  <testcase name="drains" classname="com.acme.OrderTest" time="0.10"/>
  <testcase name="rejects" classname="com.acme.OrderTest" time="0.12">
    <failure message="expected 400 &amp; got 500" type="AssertionError"/>
  </testcase>
  <testcase name="windows only" classname="com.acme.OrderTest" time="0">
    <skipped/>
  </testcase>
</testsuite>"#;

    #[test]
    fn parses_the_stable_core_of_a_surefire_document() {
        let mut cases = Vec::new();
        parse_document(SUREFIRE, "r.xml", &mut cases).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(
            (cases[0].suite.as_str(), cases[0].name.as_str(), cases[0].outcome),
            ("com.acme.OrderTest", "drains", "passed")
        );
        assert_eq!(cases[0].time_ms, Some(100));
        assert_eq!(cases[1].outcome, "failed");
        // XML entities decode — the escaping rules are why this is a crate, not a hand-roll.
        assert_eq!(cases[1].message.as_deref(), Some("expected 400 & got 500"));
        assert_eq!(cases[2].outcome, "skipped");
    }

    /// The pytest dialect: message as element text, cases under nested <testsuites>.
    #[test]
    fn tolerates_nested_suites_and_text_bodied_failures() {
        let xml = r#"<testsuites><testsuite name="pytest">
          <testcase name="test_ok" time="0.01"></testcase>
          <testcase name="test_boom"><failure>AssertionError: nope
trace line</failure></testcase>
        </testsuite></testsuites>"#;
        let mut cases = Vec::new();
        parse_document(xml, "py.xml", &mut cases).unwrap();
        assert_eq!(cases.len(), 2);
        // No classname → the enclosing suite's name.
        assert_eq!(cases[1].suite, "pytest");
        assert_eq!(cases[1].outcome, "failed");
        // First line only — a message is a label, not a traceback.
        assert_eq!(cases[1].message.as_deref(), Some("AssertionError: nope"));
    }

    /// A resumed deputy's account: the re-run case replaces its prior red, every other case is
    /// carried and MARKED, an origin already carried is kept, a red nobody re-ran stays red — and
    /// the merged document parses back to exactly that.
    #[test]
    fn a_merge_carries_marked_verdicts_and_replaces_what_was_re_run() {
        let prior = r#"<testsuites><testsuite name="cos-daemon">
          <testcase name="tests::ok" classname="cos-daemon" time="0.5"/>
          <testcase name="tests::flaky" classname="cos-daemon"><failure message="timed out &amp; gone"/></testcase>
          <testcase name="tests::still_red" classname="cos-daemon"><failure message="real"/></testcase>
          <testcase name="tests::older" classname="cos-daemon"><properties><property name="prova.reused_from" value="run-0"/></properties></testcase>
        </testsuite></testsuites>"#;
        let rerun = r#"<testsuites><testsuite name="cos-daemon">
          <testcase name="tests::flaky" classname="cos-daemon" time="0.2"/>
        </testsuite></testsuites>"#;
        let mut p = Vec::new();
        parse_document(prior, "prior.xml", &mut p).unwrap();
        assert_eq!(p[3].reused_from.as_deref(), Some("run-0"), "the property parses");
        let mut r = Vec::new();
        parse_document(rerun, "rerun.xml", &mut r).unwrap();

        let merged = merge_accounts(p, r, "run-1");
        let mut back = Vec::new();
        parse_document(&merged, "merged.xml", &mut back).unwrap();
        let by = |name: &str| back.iter().find(|c| c.name == name).unwrap();
        assert_eq!(back.len(), 4);
        assert_eq!((by("tests::ok").outcome, by("tests::ok").reused_from.as_deref()), ("passed", Some("run-1")));
        assert_eq!(by("tests::ok").time_ms, Some(500), "timing survives the round trip");
        assert_eq!((by("tests::flaky").outcome, by("tests::flaky").reused_from.as_deref()), ("passed", None), "re-run: conducted here, unmarked");
        assert_eq!(by("tests::still_red").outcome, "failed", "a red nobody re-ran stays red");
        assert_eq!(by("tests::still_red").message.as_deref(), Some("real"));
        assert_eq!(by("tests::older").reused_from.as_deref(), Some("run-0"), "an older origin is kept");
    }

    #[test]
    fn malformed_xml_is_a_loud_error_never_an_empty_green_report() {
        let mut cases = Vec::new();
        let err = parse_document("<testsuite><testcase", "bad.xml", &mut cases).unwrap_err();
        assert!(err.contains("bad.xml"), "{err}");
    }
}
