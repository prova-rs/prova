//! The suite runner: discover **suites** and run them across a pool of worker threads, each with
//! **its own Lua state** — true multi-core parallelism, with the *suite* as the dispatched unit.
//!
//! `mlua::Lua` (and the `Function` bodies collected from a file) are `!Send`, so a body collected on
//! one state cannot execute on another thread. The unit of one state is therefore the **suite**: a
//! directory's `suite.lua` groups its `*_test.lua` into a suite whose files load into one state, so
//! `Scope.Suite` fixtures are live cached values shared across them (no cross-VM serialization). An
//! ungrouped file is a *singleton* suite — one file, no setup — so the default is exactly per-file
//! parallelism, unchanged. Suites run in parallel; within a suite, cooperative async as before.
//!
//! `discover_suites` builds the grouping; `run_suites` dispatches (inline for one suite / `--jobs 1`,
//! else a worker pool draining a suite queue). The per-state loading + combined plan + per-file
//! `Scope.File` reset + one suite teardown live in `engine::run_suite_files`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{run_suite_files, RunConfig};
use crate::model::{Event, Outcome, Reporter, Summary};

/// A **suite**: files that share one Lua state (so `Scope.Suite` fixtures are built once and shared
/// across them). `setup` is an optional `suite.lua` run first (where suite-scoped fixtures live). A
/// lone ungrouped file is a *singleton* suite — one file, no setup — which behaves exactly as before.
#[derive(Clone)]
pub struct Suite {
    pub name: String,
    pub setup: Option<PathBuf>,
    pub files: Vec<PathBuf>,
}

impl Suite {
    fn singleton(file: PathBuf) -> Suite {
        let name = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("tests")
            .to_string();
        Suite {
            name,
            setup: None,
            files: vec![file],
        }
    }

    fn run(&self, reporter: &mut dyn Reporter, config: &RunConfig) -> mlua::Result<Summary> {
        run_suite_files(
            &self.name,
            self.setup.as_deref(),
            &self.files,
            reporter,
            config,
        )
    }
}

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

/// Group the tree under `root` into suites: a directory containing a `suite.lua` becomes one suite
/// owning **all** the `*_test.lua` files in its subtree (with `suite.lua` as setup); every other test
/// file is its own singleton suite. A plain file argument is a singleton suite. Sorted for
/// determinism.
pub fn discover_suites(root: &Path) -> std::io::Result<Vec<Suite>> {
    if root.is_file() {
        return Ok(vec![Suite::singleton(root.to_path_buf())]);
    }
    let mut suites = Vec::new();
    collect_suites(root, &mut suites)?;
    suites.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(suites)
}

fn collect_suites(dir: &Path, out: &mut Vec<Suite>) -> std::io::Result<()> {
    let setup = dir.join("suite.lua");
    let grouped = setup.is_file();

    // Read this directory once, partitioning its own test files from its subdirectories.
    let mut own_tests = Vec::new();
    let mut subdirs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if is_test_file(&path) {
            own_tests.push(path);
        }
    }
    own_tests.sort();
    subdirs.sort();

    // Directory-scoped: a `suite.lua` owns the `*_test.lua` in THIS directory only — not the subtree.
    // Subdirectories are always discovered independently, so a nested `suite.lua` is its own suite
    // rather than being swallowed. Sharing across directories is `require`, never silent inheritance.
    if grouped {
        if !own_tests.is_empty() {
            let name = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("suite")
                .to_string();
            out.push(Suite {
                name,
                setup: Some(setup),
                files: own_tests,
            });
        }
    } else {
        // Ungrouped: each test file is its own singleton suite.
        for t in own_tests {
            out.push(Suite::singleton(t));
        }
    }
    for sub in subdirs {
        collect_suites(&sub, out)?;
    }
    Ok(())
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
    let suites: Vec<Suite> = files.iter().cloned().map(Suite::singleton).collect();
    run_suites(&suites, reporter, config)
}

/// Run a set of suites and stream results to `reporter`. Emits one `RunStarted`/`RunFinished` for the
/// whole run; per-file node events flow through in between. **`--jobs` = concurrent suites**: one
/// suite, or `concurrency == 1`, runs inline; otherwise a pool of worker threads (each its own Lua
/// state) runs suites in parallel. Files *within* a suite share that suite's one state.
pub fn run_suites(
    suites: &[Suite],
    reporter: &mut dyn Reporter,
    config: &RunConfig,
) -> mlua::Result<Summary> {
    let started = Instant::now();
    reporter.event(&Event::RunStarted);

    let mut summary = if config.concurrency <= 1 || suites.len() <= 1 {
        run_sequential(suites, reporter, config)
    } else {
        run_pooled(suites, reporter, config)
    };

    summary.duration = started.elapsed();
    reporter.event(&Event::RunFinished { summary: &summary });
    Ok(summary)
}

/// Suites run one after another on this thread; node events go straight to the real reporter.
fn run_sequential(suites: &[Suite], reporter: &mut dyn Reporter, config: &RunConfig) -> Summary {
    let mut summary = Summary::default();
    for suite in suites {
        match suite.run(reporter, config) {
            Ok(s) => {
                summary.passed += s.passed;
                summary.failed += s.failed;
                summary.skipped += s.skipped;
                summary.deselected += s.deselected;
            }
            Err(err) => report_suite_error(reporter, &mut summary, suite, &err.to_string()),
        }
    }
    summary
}

/// A pool of worker threads, each with its own Lua state, draining a shared **suite** queue. The
/// coordinator (this thread) forwards their owned events to the real reporter and tallies.
fn run_pooled(suites: &[Suite], reporter: &mut dyn Reporter, config: &RunConfig) -> Summary {
    let workers = config.concurrency.min(suites.len()).max(1);
    let queue: Arc<Mutex<VecDeque<Suite>>> = Arc::new(Mutex::new(suites.iter().cloned().collect()));
    let (tx, rx) = channel::<OwnedEvent>();
    // Deselected leaves emit no node events, so their count travels on a side channel.
    let (dtx, drx) = channel::<usize>();

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let queue = queue.clone();
        let tx = tx.clone();
        let dtx = dtx.clone();
        // Suite-level parallelism is the worker pool; within a suite, cooperative async as before.
        let config = config.clone();
        handles.push(std::thread::spawn(move || {
            let mut sink = ChannelReporter { tx };
            loop {
                let next = queue.lock().expect("queue mutex").pop_front();
                let Some(suite) = next else { break };
                match suite.run(&mut sink, &config) {
                    Ok(s) => {
                        let _ = dtx.send(s.deselected);
                    }
                    Err(err) => {
                        // Surface a collection/load error as a synthetic failed node for the suite.
                        let path = suite.name.clone();
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
            }
        }));
    }
    // Drop our own senders so the receivers close once every worker has finished.
    drop(tx);
    drop(dtx);

    let mut summary = Summary::default();
    while let Ok(event) = rx.recv() {
        forward(reporter, &mut summary, event);
    }
    for handle in handles {
        let _ = handle.join();
    }
    summary.deselected += drx.iter().sum::<usize>();
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

fn report_suite_error(
    reporter: &mut dyn Reporter,
    summary: &mut Summary,
    suite: &Suite,
    message: &str,
) {
    reporter.event(&Event::NodeStarted { path: &suite.name });
    summary.tally(Outcome::Failed);
    reporter.event(&Event::NodeFinished {
        path: &suite.name,
        outcome: Outcome::Failed,
        duration: Duration::ZERO,
        assertions: 0,
        message: Some(message),
    });
}
