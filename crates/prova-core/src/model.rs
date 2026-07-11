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
                    "\n{} passed, {} failed, {} skipped   in {:.1?}",
                    summary.passed, summary.failed, summary.skipped, summary.duration
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
        } => json!({
            "type": "node_finished",
            "path": path,
            "outcome": outcome_str(*outcome),
            "durationMs": duration.as_secs_f64() * 1000.0,
            "assertions": assertions,
            "message": message,
        }),
        Event::RunFinished { summary } => json!({
            "type": "run_finished",
            "passed": summary.passed,
            "failed": summary.failed,
            "skipped": summary.skipped,
            "durationMs": summary.duration.as_secs_f64() * 1000.0,
        }),
    }
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
