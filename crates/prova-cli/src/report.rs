//! Presentation layer for the run path: the rich human console reporter and the knobs that
//! control it (`--color`, `--quiet`).
//!
//! prova-core stays unstyled — its `ConsoleReporter` is the minimal plain fallback — and this
//! module owns everything a human-facing terminal wants: color, source locations, skip reasons,
//! and an end-of-run failures recap so a failure that scrolled away is re-stated where the eye
//! lands last. Output degrades to the identical text with no escape codes when piped or when
//! `NO_COLOR` is set (`anstream::AutoStream` does the detection), so scripts and proofs that
//! capture stdout see plain text without asking for it.

use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::time::Duration;

use anstream::AutoStream;
use anstyle::{AnsiColor, Style};
use prova_core::{spec_summary_segment, Event, Outcome, Reporter};

/// How to color stdout. Resolution order for the run path: `--color` flag > `PROVA_COLOR` env >
/// manifest `color` key > `Auto`. Under `Auto`, anstream additionally honors `NO_COLOR` and
/// `CLICOLOR_FORCE`, and never styles a non-terminal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn parse(s: &str) -> Option<ColorMode> {
        match s {
            "auto" => Some(ColorMode::Auto),
            "always" => Some(ColorMode::Always),
            "never" => Some(ColorMode::Never),
            _ => None,
        }
    }

    fn choice(self) -> anstream::ColorChoice {
        match self {
            ColorMode::Auto => anstream::ColorChoice::Auto,
            ColorMode::Always => anstream::ColorChoice::Always,
            ColorMode::Never => anstream::ColorChoice::Never,
        }
    }
}

/// Whether to add the GitHub Actions sink (workflow-command annotations + step summary).
/// Resolution order: `--gha` flag > `PROVA_GHA` env > manifest `github` key > `Auto`. Under
/// `Auto` the sink turns on exactly when `GITHUB_ACTIONS=true` — zero-config in CI, silent
/// everywhere else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GhaMode {
    Auto,
    On,
    Off,
}

impl GhaMode {
    pub fn parse(s: &str) -> Option<GhaMode> {
        match s {
            "auto" => Some(GhaMode::Auto),
            "on" => Some(GhaMode::On),
            "off" => Some(GhaMode::Off),
            _ => None,
        }
    }

    pub fn enabled(self) -> bool {
        match self {
            GhaMode::On => true,
            GhaMode::Off => false,
            GhaMode::Auto => std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
        }
    }
}

const PASS: Style = AnsiColor::Green.on_default();
const FAIL: Style = AnsiColor::Red.on_default().bold();
const SKIP: Style = AnsiColor::Yellow.on_default();
const DIM: Style = Style::new().dimmed();
const FILE: Style = Style::new().bold();

/// A failure held back for the end-of-run recap.
struct Recap {
    path: String,
    message: Option<String>,
    location: Option<String>,
}

/// The human console reporter: a streaming **tree** — file headers, then group/flow headers,
/// then colored `PASS`/`FAIL`/`SKIP` leaves — with skip reasons inline and a `failures:` recap
/// before the summary so the red is findable without scrolling. The marks stay words (never
/// glyphs) and the text is identical uncolored, so grep and piped captures keep working.
///
/// The tree is rendered by *transition*: each leaf prints only the header levels that differ
/// from the previously printed chain. A sequential run (the default) therefore renders each
/// file and group exactly once; a parallel run (`-j>1`), whose suites interleave through one
/// coordinator channel, honestly reprints a header when output re-enters a file — never
/// buffered, so long runs keep their live feedback. Headers print lazily (only above a leaf
/// that actually prints), so `--quiet` shows exactly the chains that lead to failures.
pub struct HumanReporter {
    out: AutoStream<Stdout>,
    /// Suppress `PASS`/`SKIP` lines (`--quiet`): failures, the recap, and the summary remain.
    quiet: bool,
    /// Base for displaying source paths (the manifest home, else the cwd) — locations render
    /// relative to it when they are under it, absolute otherwise.
    rel_root: PathBuf,
    failures: Vec<Recap>,
    /// The header chain (file, then group segments) most recently printed.
    sections: Vec<String>,
    /// Whether anything has printed yet (drives the blank line between file sections).
    printed_any: bool,
}

impl HumanReporter {
    pub fn new(color: ColorMode, quiet: bool, rel_root: PathBuf) -> Self {
        Self {
            out: AutoStream::new(std::io::stdout(), color.choice()),
            quiet,
            rel_root,
            failures: Vec::new(),
            sections: Vec::new(),
            printed_any: false,
        }
    }

    /// The file path relativized against `rel_root` when it lives under it.
    fn rel_file(&self, file: &str) -> String {
        std::path::Path::new(file)
            .strip_prefix(&self.rel_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| file.to_string())
    }

    /// `file:line` for the recap, relativized like the headers.
    fn location(&self, file: Option<&str>, line: Option<u32>) -> Option<String> {
        let file = file?;
        let shown = self.rel_file(file);
        Some(match line {
            Some(line) => format!("{shown}:{line}"),
            None => shown,
        })
    }

    /// The header chain a leaf sits under — its file, then its ancestor group/flow segments —
    /// plus the leaf's own name. In a multi-file suite each file's nodes are wrapped in a
    /// file-group named after the file's stem; that segment duplicates the file header, so it
    /// is folded away.
    fn sections_for<'p>(&self, path: &'p str, file: Option<&str>) -> (Vec<String>, &'p str) {
        let mut segments: Vec<&str> = path.split(" › ").collect();
        let leaf = segments.pop().unwrap_or(path);
        let mut sections: Vec<String> = Vec::new();
        if let Some(file) = file {
            sections.push(self.rel_file(file));
            if let Some(first) = segments.first() {
                let stem = std::path::Path::new(file)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if *first == stem {
                    segments.remove(0);
                }
            }
        }
        sections.extend(segments.iter().map(|s| s.to_string()));
        (sections, leaf)
    }

    /// Print the header levels of `sections` that differ from the last printed chain — the
    /// transition rendering that turns the flat event stream into a tree.
    fn enter(&mut self, sections: &[String], has_file: bool) {
        let common = self
            .sections
            .iter()
            .zip(sections)
            .take_while(|(a, b)| a == b)
            .count();
        for (level, section) in sections.iter().enumerate().skip(common) {
            let indent = "  ".repeat(level);
            if level == 0 && has_file {
                if self.printed_any {
                    let _ = writeln!(self.out);
                }
                let _ = writeln!(self.out, "{indent}{FILE}{section}{FILE:#}");
            } else {
                let _ = writeln!(self.out, "{indent}{section}");
            }
            self.printed_any = true;
        }
        self.sections = sections.to_vec();
    }
}

/// `2.0ms`-style rendering (Duration's alternate debug form, precision 1).
fn dur(d: Duration) -> String {
    format!("{d:.1?}")
}

/// Print a (possibly multi-line) failure message under its head line: first line behind `↳ `,
/// continuation lines aligned beneath it — a snapshot diff stays a readable block.
fn write_message(out: &mut impl std::io::Write, indent: &str, message: &str) {
    let mut lines = message.lines();
    if let Some(first) = lines.next() {
        let _ = writeln!(out, "{indent}↳ {first}");
    }
    for rest in lines {
        let _ = writeln!(out, "{indent}  {rest}");
    }
}

/// The GitHub Actions sink: an *additional* reporter (composes with any `--format`) that turns
/// failures into `::error` workflow commands — inline PR annotations on the failing test's
/// file:line — skips into `::notice` commands, and the whole run into a markdown table appended
/// to `$GITHUB_STEP_SUMMARY`.
///
/// Workflow commands go to stdout by contract (GitHub strips them from the rendered log), and
/// annotation paths must be relative to `$GITHUB_WORKSPACE` — not the cwd — or they silently
/// fail to attach when the workflow sets a `working-directory`.
///
/// Deliberately NO per-suite `::group::` folding: parallel suites interleave their events
/// through one coordinator channel, so groups would scramble. Findability is carried by the
/// annotations and the step-summary table instead.
pub struct GitHubReporter {
    /// Base annotation paths are made relative to (`$GITHUB_WORKSPACE`, else the cwd).
    workspace: PathBuf,
    /// `$GITHUB_STEP_SUMMARY` — the file GitHub renders as the job's summary panel.
    step_summary: Option<PathBuf>,
    /// Non-passed results, buffered for the summary table.
    rows: Vec<SummaryRow>,
}

struct SummaryRow {
    mark: &'static str, // ❌ / ⏭️
    path: String,
    location: Option<String>,
    detail: String,
}

impl GitHubReporter {
    /// Build from the Actions environment (`GITHUB_WORKSPACE`, `GITHUB_STEP_SUMMARY`).
    pub fn from_env() -> Self {
        Self {
            workspace: std::env::var_os("GITHUB_WORKSPACE")
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from(".")),
            step_summary: std::env::var_os("GITHUB_STEP_SUMMARY").map(PathBuf::from),
            rows: Vec::new(),
        }
    }

    fn rel(&self, file: &str) -> String {
        std::path::Path::new(file)
            .strip_prefix(&self.workspace)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| file.to_string())
    }

    /// `file=...,line=...,title=...` — the annotation's property list, empty when no location.
    fn props(&self, file: Option<&str>, line: Option<u32>, title: &str) -> String {
        let mut props: Vec<String> = Vec::new();
        if let Some(file) = file {
            props.push(format!("file={}", gha_escape_property(&self.rel(file))));
            if let Some(line) = line {
                props.push(format!("line={line}"));
            }
        }
        props.push(format!("title={}", gha_escape_property(title)));
        props.join(",")
    }
}

/// Escape workflow-command *data* (the message after `::`): `%`, CR, LF.
fn gha_escape_data(s: &str) -> String {
    s.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

/// Escape a workflow-command *property* value: data escapes plus `:` and `,`.
fn gha_escape_property(s: &str) -> String {
    gha_escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// A markdown table cell: newlines flattened, pipes escaped, long messages truncated (the full
/// text is in the log and the annotation; the table is a map, not the territory).
fn summary_cell(s: &str) -> String {
    const MAX: usize = 200;
    let flat = s.replace('\n', " ").replace('|', "\\|");
    match flat.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}

impl Reporter for GitHubReporter {
    fn event(&mut self, event: &Event) {
        match event {
            Event::NodeFinished {
                path,
                outcome,
                message,
                file,
                line,
                ..
            } => match outcome {
                Outcome::Failed => {
                    let detail = message.unwrap_or("test failed");
                    println!(
                        "::error {}::{}",
                        self.props(*file, *line, path),
                        gha_escape_data(detail)
                    );
                    self.rows.push(SummaryRow {
                        mark: "❌",
                        path: path.to_string(),
                        location: file.map(|f| {
                            let f = self.rel(f);
                            match line {
                                Some(l) => format!("{f}:{l}"),
                                None => f,
                            }
                        }),
                        detail: detail.to_string(),
                    });
                }
                Outcome::Skipped => {
                    let detail = message.unwrap_or("skipped");
                    println!(
                        "::notice {}::{}",
                        self.props(*file, *line, path),
                        gha_escape_data(detail)
                    );
                    self.rows.push(SummaryRow {
                        mark: "⏭️",
                        path: path.to_string(),
                        location: file.map(|f| {
                            let f = self.rel(f);
                            match line {
                                Some(l) => format!("{f}:{l}"),
                                None => f,
                            }
                        }),
                        detail: detail.to_string(),
                    });
                }
                Outcome::Spec => {
                    // An open spec is a notice, never an error — the whole point is a green CI
                    // while the spec surface burns down. And it is EXPECTED red: first error
                    // line only, no traceback noise (the console reporter's rule holds in
                    // annotations too).
                    let detail = message
                        .map(|m| m.lines().next().unwrap_or(m))
                        .unwrap_or("open spec");
                    println!(
                        "::notice {}::{}",
                        self.props(*file, *line, path),
                        gha_escape_data(detail)
                    );
                    self.rows.push(SummaryRow {
                        mark: "📝",
                        path: path.to_string(),
                        location: file.map(|f| {
                            let f = self.rel(f);
                            match line {
                                Some(l) => format!("{f}:{l}"),
                                None => f,
                            }
                        }),
                        detail: detail.to_string(),
                    });
                }
                Outcome::Passed => {}
            },
            Event::RunFinished { summary } => {
                let Some(path) = &self.step_summary else {
                    return;
                };
                let mut md = String::new();
                let mark = if summary.failed > 0 { "❌" } else { "✅" };
                md.push_str(&format!(
                    "### {mark} prova — {} passed, {} failed, {} skipped{} in {:.1?}\n",
                    summary.passed,
                    summary.failed,
                    summary.skipped,
                    spec_summary_segment(summary),
                    summary.duration
                ));
                if !self.rows.is_empty() {
                    md.push_str("\n| | test | location | detail |\n|---|---|---|---|\n");
                    for row in &self.rows {
                        md.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            row.mark,
                            summary_cell(&row.path),
                            row.location
                                .as_deref()
                                .map(|l| format!("`{l}`"))
                                .unwrap_or_default(),
                            summary_cell(&row.detail)
                        ));
                    }
                }
                // Append, never truncate — other steps share this file.
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut f| f.write_all(md.as_bytes()));
            }
            _ => {}
        }
    }
}

impl Reporter for HumanReporter {
    fn event(&mut self, event: &Event) {
        match event {
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
                // Quiet suppresses the routine (PASS/SKIP); an open SPEC is actionable state and
                // stays visible.
                if matches!(outcome, Outcome::Passed | Outcome::Skipped) && self.quiet {
                    return;
                }
                let (sections, leaf) = self.sections_for(path, *file);
                self.enter(&sections, file.is_some());
                // Leaves indent one level below their headers (min one level, so a header-less
                // leaf — a file-less run — still reads as a row, not a heading).
                let indent = "  ".repeat(sections.len().max(1));
                // `:line` only — the file is the section header above.
                let line_col = line
                    .map(|l| format!("  {DIM}:{l}{DIM:#}"))
                    .unwrap_or_default();
                let location = self.location(*file, *line);
                let out = &mut self.out;
                match outcome {
                    Outcome::Passed => {
                        let n = *assertions;
                        let plural = if n == 1 { "assert" } else { "asserts" };
                        let _ = writeln!(
                            out,
                            "{indent}{PASS}PASS{PASS:#}  {leaf}  {DIM}({}, {n} {plural}){DIM:#}{line_col}",
                            dur(*duration)
                        );
                    }
                    Outcome::Skipped => {
                        let reason = message
                            .map(|m| format!("  {DIM}— {m}{DIM:#}"))
                            .unwrap_or_default();
                        let _ = writeln!(out, "{indent}{SKIP}SKIP{SKIP:#}  {leaf}{reason}{line_col}");
                    }
                    Outcome::Failed => {
                        let _ = writeln!(
                            out,
                            "{indent}{FAIL}FAIL{FAIL:#}  {leaf}  {DIM}({}){DIM:#}{line_col}",
                            dur(*duration)
                        );
                        if let Some(m) = message {
                            write_message(out, &format!("{indent}      "), m);
                        }
                        self.failures.push(Recap {
                            path: path.to_string(),
                            message: message.map(str::to_string),
                            location,
                        });
                    }
                    Outcome::Spec => {
                        // An open spec: expected-red, so no recap entry and no rerun line — the
                        // burndown meter (`--specs --list`, the tally) is its call to action.
                        let why = spec_reason
                            .filter(|r| !r.is_empty())
                            .map(|r| format!("  {DIM}— {r}{DIM:#}"))
                            .unwrap_or_default();
                        let _ = writeln!(out, "{indent}{SKIP}SPEC{SKIP:#}  {leaf}{why}{line_col}");
                        // First line only: the error is the call to action, but an EXPECTED
                        // failure carries no traceback noise. `--strict-specs` (where an open
                        // spec is being actively worked) reports it as FAIL with full detail.
                        if let Some(first) = message.and_then(|m| m.lines().next()) {
                            write_message(out, &format!("{indent}      "), first);
                        }
                    }
                }
                self.printed_any = true;
            }
            Event::RunFinished { summary } => {
                let out = &mut self.out;
                // The recap: every failure re-stated at the end, where the eye lands — with a
                // copy-pasteable exact-node rerun line, so "find the red again" is never a grep.
                if !self.failures.is_empty() {
                    let _ = writeln!(out, "\n{FAIL}failures:{FAIL:#}");
                    for f in &self.failures {
                        let _ = write!(out, "\n  {FAIL}FAIL{FAIL:#}  {}", f.path);
                        if let Some(loc) = &f.location {
                            let _ = write!(out, "  {DIM}{loc}{DIM:#}");
                        }
                        let _ = writeln!(out);
                        if let Some(m) = &f.message {
                            write_message(out, "        ", m);
                        }
                        let _ = writeln!(
                            out,
                            "        {DIM}rerun: prova --node {:?}{DIM:#}",
                            f.path
                        );
                    }
                }
                // The tally sentence keeps its exact uncolored text (proofs and scripts match on
                // "N passed"); color only flags the failed count, and only when it is non-zero.
                let failed_style = if summary.failed > 0 { FAIL } else { Style::new() };
                let _ = writeln!(
                    out,
                    "\n{PASS}{}{PASS:#} passed, {failed_style}{}{failed_style:#} failed, {} skipped{}{}   in {}",
                    summary.passed,
                    summary.failed,
                    summary.skipped,
                    spec_summary_segment(summary),
                    if summary.deselected > 0 {
                        format!(", {} deselected", summary.deselected)
                    } else {
                        String::new()
                    },
                    dur(summary.duration)
                );
            }
            _ => {}
        }
    }
}
