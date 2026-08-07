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
    /// An **open spec**: a leaf under a `spec` flag whose body is (expectedly) red — a proof
    /// authored ahead of its implementation. Distinct from `Failed` so CI stays green while the
    /// spec is open; a spec'd leaf that *passes* is reported as `Failed` demanding graduation
    /// (`spec = false`), so the flag can never outlive its implementation. See
    /// docs/plans/api-freeze.md §5.
    Spec,
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
/// exclusive (writer) hold — `prova.writes`; `shared = true` is a concurrent reader — `prova.reads`.
/// The scheduler uses these to co-schedule the parallelizable set so declared resources never
/// collide.
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
    /// The `spec` flag — **test-level only**: `Some(reason)` marks a proof authored ahead of its
    /// implementation. The reason is always a non-empty string — context is forced from day
    /// one, and it graduates into the `proves` context. Red body → the `Spec` outcome; green
    /// body → a failure demanding graduation (convert the flag to `proves` or remove it). A test
    /// either carries the flag or it is a full proof — there is no inheritance and no
    /// `spec = false`.
    pub spec: Option<String>,
    /// `covers` — addresses of the external obligations this proof discharges (a doc-claim anchor,
    /// a ticket). Test-level, like `spec`: an obligation is discharged by an assertion, and a
    /// container has none of its own. Plain data, so it lives here rather than beside the body.
    pub covers: Vec<String>,
    /// The `proves` attribute — **test-level only**: graduated context. `Some(context)` carries
    /// the why behind a finished proof (usually the converted reason of a spec that graduated,
    /// or retrofitted onto an existing test) in the test itself, where a reviewer cannot miss
    /// it. Runtime-inert — pass is pass, fail is fail — and mutually exclusive with `spec`:
    /// while the work is open its context lives in the spec flag.
    pub proves: Option<String>,
}

/// One reminder's evaluated state — the attention account's vocabulary (docs/design/reminders.md).
///
/// Deliberately NOT an [`Outcome`]: a reminder is not a test, never enters the run tally, and its
/// states answer a different question ("does the world owe us attention?") than pass/fail answers
/// ("is the system correct?"). Two accounts, strictly separated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReminderState {
    /// The condition evaluated false — armed and observing, the world holds still. Silence in
    /// the run output: a watcher with nothing to report says nothing.
    Watching,
    /// The condition evaluated true — attention is owed. `why` is the condition's own report of
    /// what the world did (a truthy string return), when it gave one.
    Due { why: Option<String> },
    /// The condition could not run (capability absent, or it raised). Never impersonates
    /// `Watching`: a tripwire that could not look is not a tripwire that saw nothing.
    Unevaluated { reason: String },
}

/// One reminder, evaluated: identity, instruction, declaration site, and state.
#[derive(Debug, Clone)]
pub struct ReminderOutcome {
    pub name: String,
    /// The instruction — what to DO when this fires. A reminder's discharge is an act, so this is
    /// what it carries instead of an assertion.
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub state: ReminderState,
}

/// The read-only view of this run's account that a reminder condition receives — evaluated after
/// the proof phase, so "all proofs green" / "all promises fulfilled" / "nothing owed" are one-line
/// conditions. Carries NO reminder state: reminders cannot observe reminders (one pass, one
/// direction — evidence first, attention second).
#[derive(Debug, Clone, Default)]
pub struct ReminderAccount {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Open promises in this run (the `promised` count of the headline).
    pub promised: usize,
    /// The obligation ledger's remainder — open promises, unproven claims, dangling covers — as
    /// `prova owed` would count it. Supplied by the caller (the reconciliation lives CLI-side).
    pub owed: usize,
    /// This run's measurements (name → value), so a condition can read the same scalar a ratchet
    /// gates on — the pre-authorship (future) surface of the claim the proof adjudicates in the past
    /// (docs/design/verifiers.md). Exposed to the `when` closure as `account.measurements[name]`.
    pub measurements: Vec<(String, f64)>,
}

/// One deputed case — a verdict another verifier produced, conducted and adopted by a proof
/// (docs/design/verifiers.md). Never a node: the deputy owns selection, re-runs, and
/// falsification; prova owns the account.
#[derive(Debug, Clone)]
pub struct DeputedCase {
    /// Which verifier produced it (`"junit"` today; `"tap"`, `"tlaplus"` designed).
    pub verifier: String,
    /// The suite/class the deputy groups the case under (JUnit's `classname`).
    pub suite: String,
    pub name: String,
    /// `"passed"` | `"failed"` | `"error"` | `"skipped"` — the deputy's own vocabulary, kept.
    pub outcome: String,
    pub message: Option<String>,
    pub time_ms: Option<u64>,
    /// The artifact file the verdict was read from — the provenance that makes adoption auditable.
    pub file: String,
}

/// Where a run's ingested deputed cases accumulate — shared across workers, drained by the caller
/// into the run record. The same shape as snapshot tracking: a registry handle on the config,
/// because the ingesting call sites (Lua, any worker thread) and the consumer (the record writer)
/// are far apart.
pub type DeputedRegistry = std::sync::Arc<std::sync::Mutex<Vec<DeputedCase>>>;

/// Which direction of change is an improvement — the ratchet rule and the anti-loosening guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Smaller is better: line counts, unwraps, duplication ratio, complexity.
    LowerIsBetter,
    /// Larger is better: coverage, mutation score.
    HigherIsBetter,
}

impl Direction {
    /// The stable string form, shared by the committed baseline file and the run record row.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::LowerIsBetter => "lower_is_better",
            Direction::HigherIsBetter => "higher_is_better",
        }
    }
}

/// One measurement — a named scalar a proof recorded this run (a file's line count, a coverage
/// percentage, a lint tally), plus which way is "better". Not a verdict: a ratchet assertion turns
/// it into an Outcome by comparing it against the committed baseline (`.prova/baselines/`). The
/// registry is drained into the run record (history) and read by `--update-baseline`, the guarded
/// writer that alone may move a baseline — and only ever tighter, unless forced.
#[derive(Debug, Clone)]
pub struct Measurement {
    pub name: String,
    pub value: f64,
    pub direction: Direction,
    /// Which baseline set (`.prova/baselines/<set>.json`) this belongs to — the ownership/polyglot
    /// partition. `"default"` unless the call named one.
    pub set: String,
}

/// Where a run's recorded measurements accumulate — shared across workers, drained by the caller
/// into the run record and the baseline writer. Same shape as [`DeputedRegistry`]: the recording
/// call sites (Lua, any worker thread) and the consumers (record writer, `--update-baseline`) are
/// far apart.
pub type MeasurementRegistry = std::sync::Arc<std::sync::Mutex<Vec<Measurement>>>;

/// The enforcement posture for quality gates — the observe↔enforce dial. It is NOT a new concept:
/// it selects whether a `quality.gate` authors a **proof** (fatal, evidence account) or a
/// **reminder** (surfaces, non-fatal, attention account) — the same check, different trigger.
/// Language-agnostic; a Rust pack and a "count the lines" pack read it the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Posture {
    /// A violated gate fails the run (authored as a proof). The default — today's behavior.
    #[default]
    Enforce,
    /// A violated gate surfaces but never fails the run (authored as a reminder) — the L0 "observe"
    /// / onboarding / vibe rung.
    Observe,
}

impl Posture {
    /// The stable string form surfaced to authors as `prova.quality.posture`.
    pub fn as_str(self) -> &'static str {
        match self {
            Posture::Enforce => "enforce",
            Posture::Observe => "observe",
        }
    }

    /// Parse a `[quality] posture` / `--observe`/`--enforce` value, defaulting to `Enforce`. An
    /// unrecognized value is an error the caller surfaces (mirrors `Manage::parse`).
    pub fn parse(value: Option<&str>) -> Result<Posture, String> {
        match value {
            None | Some("enforce") => Ok(Posture::Enforce),
            Some("observe") => Ok(Posture::Observe),
            Some(other) => Err(format!(
                "invalid quality posture = {other:?} (expected \"enforce\" or \"observe\")"
            )),
        }
    }
}

/// Resolved `[quality]` configuration, surfaced to authors as the `prova.quality` table so a
/// language pack reads its thresholds/posture instead of hardcoding them. Thresholds are optional —
/// a pack falls back to its own default when a project names none.
#[derive(Debug, Clone, Default)]
pub struct QualityConfig {
    pub posture: Posture,
    /// Max source-file line count a file-size gate holds to; `None` → the pack's own default.
    pub max_file_lines: Option<u64>,
}

/// Result totals for a run.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Open specs: spec-flagged leaves whose bodies are (expectedly) red. Never counted as
    /// failures — `is_success` ignores them (CI green) unless `--strict-specs` mapped them to
    /// `failed` before tallying.
    pub spec: usize,
    /// Leaves excluded by the run's selection (`-k` / `--tags` / `--node`) — never executed,
    /// distinct from `skipped` (which ran into a gate). Zero when no selection is active.
    pub deselected: usize,
    /// The reported path of every deselected leaf. The count above answers "how many"; the run
    /// record needs "which", because a selection is the cheapest way of all to report green having
    /// tested nothing in particular.
    pub deselected_paths: Vec<String>,
    /// How many `prova.remind` declarations the run collected. Plumbing, not a tally: reminders are
    /// not tests and never enter the counts above — this exists so the caller knows whether an
    /// attention-account pass (`evaluate_reminders`) has anything to evaluate, without paying a
    /// re-collection to find out that the answer is zero (the common case).
    pub reminders_declared: usize,
    pub duration: Duration,
}

impl Summary {
    pub fn tally(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Passed => self.passed += 1,
            Outcome::Failed => self.failed += 1,
            Outcome::Skipped => self.skipped += 1,
            Outcome::Spec => self.spec += 1,
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
        /// For `Outcome::Spec`: the flag's reason string (empty when the flag was a bare `true`).
        /// `None` for every other outcome. Reporters that render the spec distinctly (TAP's
        /// `# TODO`, JUnit's skip message) read it from here; `message` stays the failure text.
        spec_reason: Option<&'a str>,
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
                    Outcome::Spec => "PROMISED",
                };
                println!("  {mark}  {path}  ({duration:.1?}, {assertions} assert)");
                if let (Outcome::Failed, Some(m)) = (outcome, message) {
                    println!("          ↳ {m}");
                }
                // An open spec is expected-red: first line only, no traceback noise.
                if let (Outcome::Spec, Some(m)) = (outcome, message) {
                    if let Some(first) = m.lines().next() {
                        println!("          ↳ {first}");
                    }
                }
            }
            Event::RunFinished { summary } => {
                println!(
                    "\n{} passed, {} failed, {} skipped{}{}   in {:.1?}",
                    summary.passed,
                    summary.failed,
                    summary.skipped,
                    spec_summary_segment(summary),
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
        Outcome::Spec => "spec",
    }
}

/// The `, N promised` summary segment — present only while promises are open.
pub fn spec_summary_segment(summary: &Summary) -> String {
    if summary.spec == 0 {
        return String::new();
    }
    format!(", {} promised", summary.spec)
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
            spec_reason,
        } => json!({
            "type": "node_finished",
            "path": path,
            "outcome": outcome_str(*outcome),
            "durationMs": duration.as_secs_f64() * 1000.0,
            "assertions": assertions,
            "message": message,
            "file": file,
            "line": line,
            "specReason": spec_reason,
        }),
        Event::RunFinished { summary } => json!({
            "type": "run_finished",
            "passed": summary.passed,
            "failed": summary.failed,
            "skipped": summary.skipped,
            "spec": summary.spec,
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
    /// The spec flag's reason, for `Outcome::Spec` cases (folded into the skip message).
    spec_reason: Option<String>,
    assertions: usize,
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
    /// Wall-clock start, captured on `RunStarted` → the `<testsuite timestamp="...">` attribute.
    started: Option<std::time::SystemTime>,
    /// `<properties>` for the suite (e.g. `prova.version`, `prova.jobs`) — run metadata dashboards
    /// display but the schema has no dedicated attribute for.
    properties: Vec<(String, String)>,
}

impl<W: Write> JUnitReporter<W> {
    /// `suite_name` names the `<testsuite>` and is the fallback `classname` for top-level leaves.
    pub fn new(writer: W, suite_name: impl Into<String>) -> Self {
        Self {
            writer,
            suite_name: suite_name.into(),
            cases: Vec::new(),
            started: None,
            properties: Vec::new(),
        }
    }

    /// Attach `<properties>` entries emitted on the `<testsuite>`.
    pub fn with_properties(mut self, properties: Vec<(String, String)>) -> Self {
        self.properties = properties;
        self
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
            Event::RunStarted => {
                self.started = Some(std::time::SystemTime::now());
            }
            Event::NodeFinished {
                path,
                outcome,
                duration,
                assertions,
                message,
                file,
                line,
                spec_reason,
            } => {
                let (classname, name) = split_classname(path, &self.suite_name);
                self.cases.push(JUnitCase {
                    name,
                    classname,
                    outcome: *outcome,
                    duration: *duration,
                    message: message.map(str::to_string),
                    spec_reason: spec_reason.map(str::to_string),
                    assertions: *assertions,
                    file: file.map(str::to_string),
                    line: *line,
                });
            }
            Event::RunFinished { summary } => {
                let w = &mut self.writer;
                let secs = |d: Duration| format!("{:.3}", d.as_secs_f64());
                let total = self.cases.len();
                // `errors="0"` on both elements: prova reports every non-pass as a <failure> (no
                // <error> distinction yet), but dashboards expect the attribute to exist.
                let _ = writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
                // Open specs count as skips at the JUnit level: not failures (CI green), but
                // visibly not-run-to-success. Their `<skipped>` message carries the spec detail.
                let skipped_attr = summary.skipped + summary.spec;
                let _ = writeln!(
                    w,
                    "<testsuites tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{}\">",
                    total,
                    summary.failed,
                    skipped_attr,
                    secs(summary.duration)
                );
                let timestamp = self
                    .started
                    .map(|t| format!(" timestamp=\"{}\"", humantime::format_rfc3339_seconds(t)))
                    .unwrap_or_default();
                let _ = writeln!(
                    w,
                    "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{}\"{timestamp}>",
                    xml_escape(&self.suite_name),
                    total,
                    summary.failed,
                    skipped_attr,
                    secs(summary.duration)
                );
                if !self.properties.is_empty() {
                    let _ = writeln!(w, "    <properties>");
                    for (name, value) in &self.properties {
                        let _ = writeln!(
                            w,
                            "      <property name=\"{}\" value=\"{}\"/>",
                            xml_escape(name),
                            xml_escape(value)
                        );
                    }
                    let _ = writeln!(w, "    </properties>");
                }
                for c in &self.cases {
                    let mut head = format!(
                        "    <testcase classname=\"{}\" name=\"{}\" time=\"{}\" assertions=\"{}\"",
                        xml_escape(&c.classname),
                        xml_escape(&c.name),
                        secs(c.duration),
                        c.assertions
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
                        Outcome::Spec => {
                            // An open spec renders as a skip whose message names it — dashboards
                            // show it as not-run-to-success without failing the build.
                            let _ = writeln!(w, "{head}>");
                            let mut msg = String::from("open spec");
                            if let Some(r) = c.spec_reason.as_deref().filter(|r| !r.is_empty()) {
                                msg.push_str(&format!(": {r}"));
                            }
                            if let Some(m) = c.message.as_deref() {
                                msg.push_str(&format!(" — {m}"));
                            }
                            let _ =
                                writeln!(w, "      <skipped message=\"{}\"/>", xml_escape(&msg));
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
                spec_reason,
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
                    Outcome::Failed | Outcome::Spec => {
                        // An open spec is TAP's `# TODO` — the directive with exactly these
                        // semantics: an expected failure consumers do not count against the run.
                        let todo = match outcome {
                            Outcome::Spec => match spec_reason {
                                Some(r) if !r.is_empty() => format!(" # TODO {r}"),
                                _ => " # TODO".to_string(),
                            },
                            _ => String::new(),
                        };
                        let _ = writeln!(self.writer, "not ok {n} - {path}{todo}");
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
    /// one skip, one open spec — driven through a reporter, capturing its bytes.
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
            spec_reason: None,
        });
        reporter.event(&Event::NodeFinished {
            path: "orders › rejects <bad> & \"quoted\"",
            outcome: Outcome::Failed,
            duration: d,
            assertions: 1,
            message: Some("expected 200 got 500 <tag> & \"q\""),
            file: Some("/proj/proofs/orders_test.lua"),
            line: Some(31),
            spec_reason: None,
        });
        reporter.event(&Event::NodeFinished {
            path: "top-level check",
            outcome: Outcome::Skipped,
            duration: Duration::ZERO,
            assertions: 0,
            message: Some("docker unavailable"),
            file: None,
            line: None,
            spec_reason: None,
        });
        reporter.event(&Event::NodeFinished {
            path: "formats › json round-trips",
            outcome: Outcome::Spec,
            duration: d,
            assertions: 1,
            message: Some("expected 2, got 1"),
            file: Some("/proj/proofs/formats_test.lua"),
            line: Some(7),
            spec_reason: Some("api-freeze §1"),
        });
        let summary = Summary {
            passed: 1,
            failed: 1,
            skipped: 1,
            spec: 1,
            deselected: 0,
            deselected_paths: Vec::new(),
            reminders_declared: 0,
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

        // Document + suite totals. The open spec counts into `skipped` (not-run-to-success,
        // never a failure), so dashboards stay green while the spec surface burns down.
        assert!(
            xml.contains(r#"<testsuites tests="4" failures="1" errors="0" skipped="2""#),
            "{xml}"
        );
        assert!(xml.contains(r#"<testsuite name="prova" tests="4" failures="1" errors="0" skipped="2""#));
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
        // An open spec renders as a skip naming the flag's reason and the red detail.
        assert!(
            xml.contains(r#"<skipped message="open spec: api-freeze §1 — expected 2, got 1"/>"#),
            "{xml}"
        );
        // Passed case is a self-closing testcase (no children), carrying its assertion count and
        // source location.
        assert!(
            xml.contains(
                r#"name="creates a row" time="0.002" assertions="1" file="/proj/proofs/orders_test.lua" line="12"/>"#
            ),
            "{xml}"
        );
        // A leaf without file backing omits the location attributes entirely.
        assert!(
            xml.contains(r#"name="top-level check" time="0.000" assertions="0">"#),
            "{xml}"
        );
        // Run metadata: a timestamp captured at RunStarted, and errors="0" for dashboards.
        assert!(xml.contains(r#" timestamp=""#), "{xml}");
        assert!(xml.contains(r#" errors="0""#), "{xml}");
    }

    #[test]
    fn junit_emits_suite_properties_when_attached() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut r = JUnitReporter::new(&mut buf, "prova").with_properties(vec![
                ("prova.version".into(), "0.4.0".into()),
                ("prova.profile".into(), "ci <&>".into()),
            ]);
            drive(&mut r);
        }
        let xml = String::from_utf8(buf).unwrap();
        assert!(xml.contains("<properties>"), "{xml}");
        assert!(
            xml.contains(r#"<property name="prova.version" value="0.4.0"/>"#),
            "{xml}"
        );
        // Property values are XML-escaped like everything else.
        assert!(
            xml.contains(r#"<property name="prova.profile" value="ci &lt;&amp;&gt;"/>"#),
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
        // An open spec is TAP's `# TODO` directive — an expected failure consumers do not count
        // against the run — with the flag's reason as the directive text and the red detail in
        // the YAML diagnostic block.
        assert_eq!(
            lines[9],
            "not ok 4 - formats › json round-trips # TODO api-freeze §1"
        );
        assert_eq!(lines[10], "  ---");
        assert_eq!(lines[11], r#"  message: "expected 2, got 1""#);
        assert_eq!(lines[12], r#"  file: "/proj/proofs/formats_test.lua""#);
        assert_eq!(lines[13], "  line: 7");
        assert_eq!(lines[14], "  ...");
        // Trailing plan reflects the count.
        assert_eq!(lines[15], "1..4");
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
