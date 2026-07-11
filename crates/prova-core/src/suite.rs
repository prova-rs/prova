//! The suite runner: discover many test files and run them across a pool of worker threads, each
//! with **its own Lua state** — the realization of true multi-core parallelism.
//!
//! Why per-*file* workers rather than per-*unit*: `mlua::Lua` (and the `Function` bodies collected
//! from a file) are `!Send`, so a body collected on one Lua state cannot execute on another thread.
//! The file is therefore the natural thread boundary — each worker loads a file into its own state
//! and runs it end to end with the in-file scheduler (deps, resources, cooperative async). Files run
//! in parallel; within a file, everything is as before.
//!
//! Scope consequences: `file` scope is naturally per-file (correct). A cross-file `suite` fixture
//! becomes per-worker under `--jobs > 1`, since a Lua value cannot cross `!Send` states — the
//! documented open question (a serialized once-guard for serializable values is future work). A
//! single file, or `--jobs 1`, preserves the exact prior single-state semantics.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{run_file_into, RunConfig};
use crate::model::{Event, Outcome, Reporter, Summary};

/// Owned counterpart of a node-level `Event`, so results can cross the worker→coordinator channel
/// (the borrowed `Event<'a>` cannot be sent). The coordinator reconstructs an `Event` from these to
/// feed the real reporter on the main thread.
enum OwnedEvent {
    NodeStarted {
        path: String,
    },
    NodeFinished {
        path: String,
        outcome: Outcome,
        duration: Duration,
        assertions: usize,
        message: Option<String>,
    },
}

/// A worker-side `Reporter` that forwards node events (owned) over a channel and drops run-level
/// events — the coordinator owns `RunStarted`/`RunFinished` for the whole suite.
struct ChannelReporter {
    tx: Sender<OwnedEvent>,
}

impl Reporter for ChannelReporter {
    fn event(&mut self, event: &Event) {
        let owned = match event {
            Event::NodeStarted { path } => OwnedEvent::NodeStarted {
                path: (*path).to_string(),
            },
            Event::NodeFinished {
                path,
                outcome,
                duration,
                assertions,
                message,
            } => OwnedEvent::NodeFinished {
                path: (*path).to_string(),
                outcome: *outcome,
                duration: *duration,
                assertions: *assertions,
                message: message.map(str::to_string),
            },
            Event::RunStarted | Event::RunFinished { .. } => return,
        };
        // A closed receiver only happens if the coordinator has already torn down; drop silently.
        let _ = self.tx.send(owned);
    }
}

/// Recursively collect test files under `root` (`*_test.lua` / `*.test.lua`), or just `[root]` if it
/// is itself a file. Results are sorted for deterministic discovery order.
pub fn discover_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if root.is_file() {
        out.push(root.to_path_buf());
        return Ok(out);
    }
    collect_dir(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn is_test_file(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name.ends_with("_test.lua") || name.ends_with(".test.lua"),
        None => false,
    }
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_dir(&path, out)?;
        } else if is_test_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Run a whole suite (one or more files) and stream results to `reporter`. Emits one `RunStarted`
/// and one `RunFinished` for the whole suite; per-file node events flow through in between.
///
/// A single file, or `config.concurrency == 1`, runs inline on this thread (no worker pool),
/// preserving the exact single-state path. Otherwise a pool of `min(jobs, files)` worker threads —
/// each with its own Lua state — pulls files off a shared queue and runs them in parallel.
pub fn run_suite(
    files: &[PathBuf],
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    let started = Instant::now();
    reporter.event(&Event::RunStarted);

    let mut summary = if config.concurrency <= 1 || files.len() <= 1 {
        run_sequential(files, reporter, config)
    } else {
        run_pooled(files, reporter, config)
    };

    summary.duration = started.elapsed();
    reporter.event(&Event::RunFinished { summary: &summary });
    Ok(summary)
}

/// Files run one after another on this thread; node events go straight to the real reporter.
fn run_sequential(files: &[PathBuf], reporter: &mut dyn Reporter, config: &RunConfig) -> Summary {
    let mut summary = Summary::default();
    for file in files {
        match run_file_into(file, reporter, config) {
            Ok(s) => {
                summary.passed += s.passed;
                summary.failed += s.failed;
                summary.skipped += s.skipped;
            }
            Err(err) => report_file_error(reporter, &mut summary, file, &err.to_string()),
        }
    }
    summary
}

/// A pool of worker threads, each with its own Lua state, draining a shared file queue. The
/// coordinator (this thread) forwards their owned events to the real reporter and tallies.
fn run_pooled(files: &[PathBuf], reporter: &mut dyn Reporter, config: &RunConfig) -> Summary {
    let workers = config.concurrency.min(files.len()).max(1);
    let queue: Arc<Mutex<VecDeque<PathBuf>>> =
        Arc::new(Mutex::new(files.iter().cloned().collect()));
    let (tx, rx) = channel::<OwnedEvent>();

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let queue = queue.clone();
        let tx = tx.clone();
        // Each file runs with in-file concurrency = jobs (cooperative I/O overlap within the file);
        // file-level parallelism is the worker pool itself.
        let config = config.clone();
        handles.push(std::thread::spawn(move || {
            let mut sink = ChannelReporter { tx };
            loop {
                let next = queue.lock().expect("queue mutex").pop_front();
                let Some(file) = next else { break };
                if let Err(err) = run_file_into(&file, &mut sink, &config) {
                    // Surface a collection/load error as a synthetic failed node for this file.
                    let path = file.to_string_lossy();
                    let message = err.to_string();
                    sink.event(&Event::NodeStarted { path: &path });
                    sink.event(&Event::NodeFinished {
                        path: &path,
                        outcome: Outcome::Failed,
                        duration: Duration::ZERO,
                        assertions: 0,
                        message: Some(&message),
                    });
                }
            }
        }));
    }
    // Drop our own sender so `rx` closes once every worker has finished and dropped its clone.
    drop(tx);

    let mut summary = Summary::default();
    while let Ok(event) = rx.recv() {
        forward(reporter, &mut summary, event);
    }
    for handle in handles {
        let _ = handle.join();
    }
    summary
}

/// Reconstruct an `Event` from an `OwnedEvent`, forward it to the reporter, and tally finishes.
fn forward(reporter: &mut dyn Reporter, summary: &mut Summary, event: OwnedEvent) {
    match event {
        OwnedEvent::NodeStarted { path } => {
            reporter.event(&Event::NodeStarted { path: &path });
        }
        OwnedEvent::NodeFinished {
            path,
            outcome,
            duration,
            assertions,
            message,
        } => {
            summary.tally(outcome);
            reporter.event(&Event::NodeFinished {
                path: &path,
                outcome,
                duration,
                assertions,
                message: message.as_deref(),
            });
        }
    }
}

fn report_file_error(
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
    file: &Path,
    message: &str,
) {
    let path = file.to_string_lossy();
    reporter.event(&Event::NodeStarted { path: &path });
    summary.tally(Outcome::Failed);
    reporter.event(&Event::NodeFinished {
        path: &path,
        outcome: Outcome::Failed,
        duration: Duration::ZERO,
        assertions: 0,
        message: Some(message),
    });
}
