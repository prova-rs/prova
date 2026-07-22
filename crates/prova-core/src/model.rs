//! Pure engine types: outcomes, node identity, options, and the reporting/event seam.
//!
//! Nothing here touches Lua, so these are the stable vocabulary the executor, reporters, and
//! (future) load/param drivers share. The event stream (below) is the seam that lets a
//! console reporter, a JUnit writer, and a load-metrics aggregator all consume execution
//! without the executor knowing about any of them.

use std::io::Write;
use std::time::Duration;

/// Index into the collection arena.
pub type NodeIx = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
}

/// The parameter bindings that make a node's identity unique.
///
/// Empty today for every node, but it is part of node identity *now* so that when parameterized
/// tests / property generators land, `foo[lang=rust]` and `foo[lang=java]` are already distinct,
/// selectable, individually-reportable units — no identity retrofit.
#[derive(Debug, Clone, Default)]
pub struct Params(pub Vec<(String, String)>);

impl Params {
    /// The `[k=v,...]` suffix appended to a node's name to disambiguate cases.
    pub fn suffix(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        let inner = self
            .0
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("[{inner}]")
    }
}

/// A declared need for an external resource, with readers-writer semantics. `shared = false` is an
/// exclusive (writer) hold; `shared = true` is a concurrent reader. The scheduler uses these to
/// co-schedule the parallelizable set so declared resources never collide.
#[derive(Debug, Clone)]
pub struct ResourceReq {
    pub token: String,
    pub shared: bool,
}

/// Per-unit options parsed from the Lua `opts` table.
///
/// `timeout` is parsed and carried now (the plumbing) even though enforcement is a later
/// increment — see the deadline seam in `engine::run_node`.
#[derive(Debug, Clone, Default)]
pub struct UnitOpts {
    pub timeout: Option<Duration>,
    pub tags: Vec<String>,
    /// Units this one depends on, as arena indices (resolved from `depends_on` handles). A unit is
    /// skipped (not failed) if any transitive dependency leaf failed or was skipped.
    pub depends_on: Vec<NodeIx>,
    /// External resources this unit needs; the scheduler gates concurrency on them.
    pub resources: Vec<ResourceReq>,
    /// Process-wide exclusive: never run concurrently with anything (sugar for an exclusive hold on
    /// a global token every other unit reads).
    pub serial: bool,
    /// Capabilities this unit needs (e.g. `"docker"`). If any is unavailable the unit is **skipped**
    /// (not failed), with a reason — so a suite degrades gracefully where a dependency is missing.
    pub requires: Vec<String>,
}

/// Result totals for a run.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Leaves excluded by the run's selection (`-k` / `--tags` / `--node`) — never executed,
    /// distinct from `skipped` (which ran into a gate). Zero when no selection is active.
    pub deselected: usize,
    pub duration: Duration,
}

impl Summary {
    pub fn tally(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Passed => self.passed += 1,
            Outcome::Failed => self.failed += 1,
            Outcome::Skipped => self.skipped += 1,
        }
    }
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

/// Structured execution events. This is the seam: the executor *emits* these; it never prints.
/// Console output, JUnit XML, TAP, and a future load-test metrics aggregator are all just
/// `Reporter` implementations over the same stream.
#[derive(Debug)]
pub enum Event<'a> {
    RunStarted,
    NodeStarted {
        path: &'a str,
    },
    NodeFinished {
        path: &'a str,
        outcome: Outcome,
        duration: Duration,
        /// Assertions executed in the body (0 → the test asserted nothing).
        assertions: usize,
        message: Option<&'a str>,
        /// Source file the leaf was declared in (absolute; reporters relativize for display).
        /// `None` when the run has no file backing (an `eval`, a topology snippet).
        file: Option<&'a str>,
        /// 1-based line of the declaration call (`prova.test(...)` / `flow:step(...)`).
        line: Option<u32>,
    },
    RunFinished {
        summary: &'a Summary,
    },
}

pub trait Reporter {
    fn event(&mut self, event: &Event);
}

/// A no-op reporter (useful for tests and for the load driver, which consumes metrics elsewhere).
pub struct NullReporter;
impl Reporter for NullReporter {
    fn event(&mut self, _event: &Event) {}
}

/// Minimal human-readable reporter.
pub struct ConsoleReporter;

impl Reporter for ConsoleReporter {
    fn event(&mut self, event: &Event) {
        match event {
            Event::NodeFinished {
                path,
                outcome,
                duration,
                assertions,
                message,
                ..
            } => {
                let mark = match outcome {
                    Outcome::Passed => "PASS",
                    Outcome::Failed => "FAIL",
                    Outcome::Skipped => "SKIP",
                };
                println!("  {mark}  {path}  ({duration:.1?}, {assertions} assert)");
                if let (Outcome::Failed, Some(m)) = (outcome, message) {
                    println!("          ↳ {m}");
                }
            }
            Event::RunFinished { summary } => {
                println!(
                    "\n{} passed, {} failed, {} skipped{}   in {:.1?}",
                    summary.passed,
                    summary.failed,
                    summary.skipped,
                    if summary.deselected > 0 {
                        format!(", {} deselected", summary.deselected)
                    } else {
                        String::new()
                    },
                    summary.duration
                );
            }
            _ => {}
        }
    }
}

/// Fan-out reporter: drive any number of sinks from one event stream. This is the plugin
/// surface for output — console + JUnit + a GUI socket can all run at once.
#[derive(Default)]
pub struct MultiReporter {
    pub sinks: Vec<Box<dyn Reporter>>,
}

impl MultiReporter {
    pub fn new(sinks: Vec<Box<dyn Reporter>>) -> Self {
        Self { sinks }
    }
    pub fn push(&mut self, sink: Box<dyn Reporter>) {
        self.sinks.push(sink);
    }
}

impl Reporter for MultiReporter {
    fn event(&mut self, event: &Event) {
        for sink in &mut self.sinks {
            sink.event(event);
        }
    }
}

/// Streaming machine protocol: one JSON object per line (JSONL). This is what a CI parser or a
/// GUI/IDE frontend consumes to render a live, model-aware view of the run.
pub struct JsonReporter<W: Write> {
    writer: W,
}

impl<W: Write> JsonReporter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: Write> Reporter for JsonReporter<W> {
    fn event(&mut self, event: &Event) {
        let _ = writeln!(self.writer, "{}", event_to_json(event));
    }
}

fn outcome_str(o: Outcome) -> &'static str {
    match o {
        Outcome::Passed => "passed",
        Outcome::Failed => "failed",
        Outcome::Skipped => "skipped",
    }
}

/// Serialize an event to a stable JSON shape (the wire protocol for frontends).
pub fn event_to_json(event: &Event) -> serde_json::Value {
    use serde_json::json;
    match event {
        Event::RunStarted => json!({ "type": "run_started" }),
        Event::NodeStarted { path } => json!({ "type": "node_started", "path": path }),
        Event::NodeFinished {
            path,
            outcome,
            duration,
            assertions,
            message,
            file,
            line,
        } => json!({
            "type": "node_finished",
            "path": path,
            "outcome": outcome_str(*outcome),
            "durationMs": duration.as_secs_f64() * 1000.0,
            "assertions": assertions,
            "message": message,
            "file": file,
            "line": line,
        }),
        Event::RunFinished { summary } => json!({
            "type": "run_finished",
            "passed": summary.passed,
            "failed": summary.failed,
            "skipped": summary.skipped,
            "deselected": summary.deselected,
            "durationMs": summary.duration.as_secs_f64() * 1000.0,
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// JUnit XML — the CI lingua franca. Buffers cases and writes one `<testsuites>` document on
// RunFinished, so it composes as a *file* sink alongside a console/tap/json stdout reporter.
// ---------------------------------------------------------------------------------------------

/// One buffered test case for the JUnit document.
struct JUnitCase {
    /// The leaf's own name (last path segment).
    name: String,
    /// Dotted ancestor path (`group.subgroup`), or the suite name for a top-level leaf — JUnit's
    /// `classname`, which CI dashboards group by.
    classname: String,
    outcome: Outcome,
    duration: Duration,
    message: Option<String>,
    /// Source location of the declaration, when the leaf has file backing — emitted as `file`/
    /// `line` attributes, which is how CI dashboards link a case back to its source.
    file: Option<String>,
    line: Option<u32>,
}

/// Writes a JUnit XML report — the format Jenkins, GitLab, GitHub Actions, CircleCI, etc. ingest to
/// render per-test results. Buffers every `NodeFinished` and emits the document on `RunFinished`.
pub struct JUnitReporter<W: Write> {
    writer: W,
    suite_name: String,
    cases: Vec<JUnitCase>,
}

impl<W: Write> JUnitReporter<W> {
    /// `suite_name` names the `<testsuite>` and is the fallback `classname` for top-level leaves.
    pub fn new(writer: W, suite_name: impl Into<String>) -> Self {
        Self {
            writer,
            suite_name: suite_name.into(),
            cases: Vec::new(),
        }
    }
}

/// Split a prova node path (`"group › test"`) into (classname, name): the last ` › ` segment is the
/// case name; the ancestors join with `.` as the classname (`fallback` when there are none).
fn split_classname(path: &str, fallback: &str) -> (String, String) {
    let mut segments: Vec<&str> = path.split(" › ").collect();
    let name = segments.pop().unwrap_or(path).trim().to_string();
    if segments.is_empty() {
        (fallback.to_string(), name)
    } else {
        (
            segments
                .iter()
                .map(|s| s.trim())
                .collect::<Vec<_>>()
                .join("."),
            name,
        )
    }
}

/// Escape the five XML predefined entities, so a test name or failure message with `<`, `&`, or a
/// quote can't corrupt the document.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

impl<W: Write> Reporter for JUnitReporter<W> {
    fn event(&mut self, event: &Event) {
        match event {
            Event::NodeFinished {
                path,
                outcome,
                duration,
                message,
                file,
                line,
                ..
            } => {
                let (classname, name) = split_classname(path, &self.suite_name);
                self.cases.push(JUnitCase {
                    name,
                    classname,
                    outcome: *outcome,
                    duration: *duration,
                    message: message.map(str::to_string),
                    file: file.map(str::to_string),
                    line: *line,
                });
            }
            Event::RunFinished { summary } => {
                let w = &mut self.writer;
                let secs = |d: Duration| format!("{:.3}", d.as_secs_f64());
                let total = self.cases.len();
                let _ = writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
                let _ = writeln!(
                    w,
                    "<testsuites tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{}\">",
                    total,
                    summary.failed,
                    summary.skipped,
                    secs(summary.duration)
                );
                let _ = writeln!(
                    w,
                    "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{}\">",
                    xml_escape(&self.suite_name),
                    total,
                    summary.failed,
                    summary.skipped,
                    secs(summary.duration)
                );
                for c in &self.cases {
                    let mut head = format!(
                        "    <testcase classname=\"{}\" name=\"{}\" time=\"{}\"",
                        xml_escape(&c.classname),
                        xml_escape(&c.name),
                        secs(c.duration)
                    );
                    if let Some(file) = &c.file {
                        head.push_str(&format!(" file=\"{}\"", xml_escape(file)));
                    }
                    if let Some(line) = c.line {
                        head.push_str(&format!(" line=\"{line}\""));
                    }
                    match c.outcome {
                        Outcome::Passed => {
                            let _ = writeln!(w, "{head}/>");
                        }
                        Outcome::Skipped => {
                            let _ = writeln!(w, "{head}>");
                            match &c.message {
                                Some(m) => {
                                    let _ = writeln!(
                                        w,
                                        "      <skipped message=\"{}\"/>",
                                        xml_escape(m)
                                    );
                                }
                                None => {
                                    let _ = writeln!(w, "      <skipped/>");
                                }
                            }
                            let _ = writeln!(w, "    </testcase>");
                        }
                        Outcome::Failed => {
                            let _ = writeln!(w, "{head}>");
                            let msg = c.message.as_deref().unwrap_or("assertion failed");
                            let _ = writeln!(
                                w,
                                "      <failure message=\"{}\">{}</failure>",
                                xml_escape(msg),
                                xml_escape(msg)
                            );
                            let _ = writeln!(w, "    </testcase>");
                        }
                    }
                }
                let _ = writeln!(w, "  </testsuite>");
                let _ = writeln!(w, "</testsuites>");
                let _ = w.flush();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// TAP (Test Anything Protocol) — a line-oriented stdout stream many harnesses and CI plugins read.
// ---------------------------------------------------------------------------------------------

/// Emits TAP version 13: a header, one `ok`/`not ok N - name` line per leaf as it finishes (with a
/// `# SKIP` directive for skips and a YAML diagnostic block for failures), and the `1..N` plan at the
/// end (a trailing plan is valid TAP and lets us stream without knowing the count up front).
pub struct TapReporter<W: Write> {
    writer: W,
    count: usize,
}

impl<W: Write> TapReporter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer, count: 0 }
    }
}

impl<W: Write> Reporter for TapReporter<W> {
    fn event(&mut self, event: &Event) {
        match event {
            Event::RunStarted => {
                let _ = writeln!(self.writer, "TAP version 13");
            }
            Event::NodeFinished {
                path,
                outcome,
                message,
                file,
                line,
                ..
            } => {
                self.count += 1;
                let n = self.count;
                match outcome {
                    Outcome::Passed => {
                        let _ = writeln!(self.writer, "ok {n} - {path}");
                    }
                    Outcome::Skipped => {
                        let reason = message.map(|m| format!(" {m}")).unwrap_or_default();
                        let _ = writeln!(self.writer, "ok {n} - {path} # SKIP{reason}");
                    }
                    Outcome::Failed => {
                        let _ = writeln!(self.writer, "not ok {n} - {path}");
                        if message.is_some() || file.is_some() {
                            // A YAML diagnostic block — TAP consumers surface it as the failure detail.
                            let _ = writeln!(self.writer, "  ---");
                            if let Some(m) = message {
                                let _ = writeln!(self.writer, "  message: {}", tap_yaml_scalar(m));
                            }
                            if let Some(f) = file {
                                let _ = writeln!(self.writer, "  file: {}", tap_yaml_scalar(f));
                            }
                            if let Some(l) = line {
                                let _ = writeln!(self.writer, "  line: {l}");
                            }
                            let _ = writeln!(self.writer, "  ...");
                        }
                    }
                }
            }
            Event::RunFinished { .. } => {
                let _ = writeln!(self.writer, "1..{}", self.count);
                let _ = self.writer.flush();
            }
            _ => {}
        }
    }
}

/// Quote a TAP diagnostic message as a single-line YAML scalar (newlines flattened), so a multi-line
/// assertion message stays inside the one `message:` key.
fn tap_yaml_scalar(s: &str) -> String {
    let flattened = s.replace('\n', " ");
    format!(
        "\"{}\"",
        flattened.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// Parse a duration micro-format: `"500ms"`, `"30s"`, `"2m"`, or a bare number (seconds).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(x) = s.strip_suffix("ms") {
        return x
            .trim()
            .parse::<f64>()
            .ok()
            .map(Duration::from_secs_f64)
            .map(|d| d / 1000);
    }
    if let Some(x) = s.strip_suffix('s') {
        return x.trim().parse::<f64>().ok().map(Duration::from_secs_f64);
    }
    if let Some(x) = s.strip_suffix('m') {
        return x
            .trim()
            .parse::<f64>()
            .ok()
            .map(|m| Duration::from_secs_f64(m * 60.0));
    }
    s.parse::<f64>().ok().map(Duration::from_secs_f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative run: one pass, one fail (with a message containing XML/TAP metacharacters),
    /// one skip — driven through a reporter, capturing its bytes.
    fn drive<R: Reporter>(reporter: &mut R) {
        let d = Duration::from_millis(2);
        reporter.event(&Event::RunStarted);
        reporter.event(&Event::NodeFinished {
            path: "orders › creates a row",
            outcome: Outcome::Passed,
            duration: d,
            assertions: 1,
            message: None,
            file: Some("/proj/proofs/orders_test.lua"),
            line: Some(12),
        });
        reporter.event(&Event::NodeFinished {
            path: "orders › rejects <bad> & \"quoted\"",
            outcome: Outcome::Failed,
            duration: d,
            assertions: 1,
            message: Some("expected 200 got 500 <tag> & \"q\""),
            file: Some("/proj/proofs/orders_test.lua"),
            line: Some(31),
        });
        reporter.event(&Event::NodeFinished {
            path: "top-level check",
            outcome: Outcome::Skipped,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some("docker unavailable"),
            file: None,
            line: None,
        });
        let summary = Summary {
            passed: 1,
            failed: 1,
            skipped: 1,
            deselected: 0,
            duration: Duration::from_millis(6),
        };
        reporter.event(&Event::RunFinished { summary: &summary });
    }

    #[test]
    fn junit_reports_cases_with_classname_split_and_xml_escaping() {
        // JUnitReporter writes on RunFinished; write into an owned Vec via a raw pointer-free path:
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = JUnitReporter::new(&mut buf, "prova");
            drive(&mut r);
        }
        let xml = String::from_utf8(buf).unwrap();

        // Document + suite totals.
        assert!(
            xml.contains(r#"<testsuites tests="3" failures="1" skipped="1""#),
            "{xml}"
        );
        assert!(xml.contains(r#"<testsuite name="prova" tests="3" failures="1" skipped="1""#));
        // Path split: ancestors → classname, leaf → name.
        assert!(
            xml.contains(r#"classname="orders" name="creates a row""#),
            "{xml}"
        );
        // Top-level leaf (no ancestors) → suite name as classname.
        assert!(
            xml.contains(r#"classname="prova" name="top-level check""#),
            "{xml}"
        );
        // Failure element + XML escaping of metacharacters in the name and message.
        assert!(
            xml.contains("&lt;bad&gt; &amp; &quot;quoted&quot;"),
            "name escaped: {xml}"
        );
        assert!(
            xml.contains(
                r#"<failure message="expected 200 got 500 &lt;tag&gt; &amp; &quot;q&quot;">"#
            ),
            "{xml}"
        );
        // Skipped element carries its reason.
        assert!(
            xml.contains(r#"<skipped message="docker unavailable"/>"#),
            "{xml}"
        );
        // Passed case is a self-closing testcase (no children), carrying its source location.
        assert!(
            xml.contains(
                r#"name="creates a row" time="0.002" file="/proj/proofs/orders_test.lua" line="12"/>"#
            ),
            "{xml}"
        );
        // A leaf without file backing omits the location attributes entirely.
        assert!(
            xml.contains(r#"name="top-level check" time="0.000">"#),
            "{xml}"
        );
    }

    #[test]
    fn tap_streams_version_results_and_trailing_plan() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = TapReporter::new(&mut buf);
            drive(&mut r);
        }
        let tap = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = tap.lines().collect();

        assert_eq!(lines[0], "TAP version 13");
        assert_eq!(lines[1], "ok 1 - orders › creates a row");
        assert_eq!(lines[2], "not ok 2 - orders › rejects <bad> & \"quoted\"");
        // Failure diagnostic YAML block: message flattened + quoted, plus the source location.
        assert_eq!(lines[3], "  ---");
        assert_eq!(
            lines[4],
            r#"  message: "expected 200 got 500 <tag> & \"q\"""#
        );
        assert_eq!(lines[5], r#"  file: "/proj/proofs/orders_test.lua""#);
        assert_eq!(lines[6], "  line: 31");
        assert_eq!(lines[7], "  ...");
        assert_eq!(lines[8], "ok 3 - top-level check # SKIP docker unavailable");
        // Trailing plan reflects the count.
        assert_eq!(lines[9], "1..3");
    }

    #[test]
    fn split_classname_handles_nesting_and_top_level() {
        assert_eq!(
            split_classname("a › b › c", "prova"),
            ("a.b".into(), "c".into())
        );
        assert_eq!(
            split_classname("solo", "prova"),
            ("prova".into(), "solo".into())
        );
    }
}
