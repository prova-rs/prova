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
use prova_core::{Event, Outcome, Reporter};

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

const PASS: Style = AnsiColor::Green.on_default();
const FAIL: Style = AnsiColor::Red.on_default().bold();
const SKIP: Style = AnsiColor::Yellow.on_default();
const DIM: Style = Style::new().dimmed();

/// A failure held back for the end-of-run recap.
struct Recap {
    path: String,
    message: Option<String>,
    location: Option<String>,
}

/// The human console reporter: colored streaming `PASS`/`FAIL`/`SKIP` lines with source
/// locations, skip reasons inline, and a `failures:` recap before the summary so the red is
/// findable without scrolling. The marks stay words (never glyphs) and the text is identical
/// uncolored, so grep and piped captures keep working.
pub struct HumanReporter {
    out: AutoStream<Stdout>,
    /// Suppress `PASS`/`SKIP` lines (`--quiet`): failures, the recap, and the summary remain.
    quiet: bool,
    /// Base for displaying source paths (the manifest home, else the cwd) — locations render
    /// relative to it when they are under it, absolute otherwise.
    rel_root: PathBuf,
    failures: Vec<Recap>,
}

impl HumanReporter {
    pub fn new(color: ColorMode, quiet: bool, rel_root: PathBuf) -> Self {
        Self {
            out: AutoStream::new(std::io::stdout(), color.choice()),
            quiet,
            rel_root,
            failures: Vec::new(),
        }
    }

    /// `file:line` with the file relativized against `rel_root` when it lives under it.
    fn location(&self, file: Option<&str>, line: Option<u32>) -> Option<String> {
        let file = file?;
        let shown = std::path::Path::new(file)
            .strip_prefix(&self.rel_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| file.to_string());
        Some(match line {
            Some(line) => format!("{shown}:{line}"),
            None => shown,
        })
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
            } => {
                let location = self.location(*file, *line);
                let out = &mut self.out;
                match outcome {
                    Outcome::Passed if !self.quiet => {
                        let n = *assertions;
                        let plural = if n == 1 { "assert" } else { "asserts" };
                        let _ = write!(
                            out,
                            "  {PASS}PASS{PASS:#}  {path}  {DIM}({}, {n} {plural}){DIM:#}",
                            dur(*duration)
                        );
                        if let Some(loc) = &location {
                            let _ = write!(out, "  {DIM}{loc}{DIM:#}");
                        }
                        let _ = writeln!(out);
                    }
                    Outcome::Passed => {}
                    Outcome::Skipped if !self.quiet => {
                        let _ = write!(out, "  {SKIP}SKIP{SKIP:#}  {path}");
                        if let Some(reason) = message {
                            let _ = write!(out, "  {DIM}— {reason}{DIM:#}");
                        }
                        if let Some(loc) = &location {
                            let _ = write!(out, "  {DIM}{loc}{DIM:#}");
                        }
                        let _ = writeln!(out);
                    }
                    Outcome::Skipped => {}
                    Outcome::Failed => {
                        let _ = write!(
                            out,
                            "  {FAIL}FAIL{FAIL:#}  {path}  {DIM}({}){DIM:#}",
                            dur(*duration)
                        );
                        if let Some(loc) = &location {
                            let _ = write!(out, "  {DIM}{loc}{DIM:#}");
                        }
                        let _ = writeln!(out);
                        if let Some(m) = message {
                            write_message(out, "        ", m);
                        }
                        self.failures.push(Recap {
                            path: path.to_string(),
                            message: message.map(str::to_string),
                            location,
                        });
                    }
                }
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
                    "\n{PASS}{}{PASS:#} passed, {failed_style}{}{failed_style:#} failed, {} skipped{}   in {}",
                    summary.passed,
                    summary.failed,
                    summary.skipped,
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
