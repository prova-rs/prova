//! The stderr activity renderer — Phase 1 of docs/plans/run-progress-feedback.md.
//!
//! Plain, threshold-gated lines telling you what a pause *is*, so a cold image pull reads as
//! provisioning rather than a wedge. Deliberately not a spinner and not a progress bar: those are
//! Phase 2, TTY-only, and they cannot be made safe for a captured pipe. What ships here works
//! identically on a terminal, through `| tee`, and in an agent's captured output.
//!
//! # The two invariants
//!
//! 1. **stderr, always.** stdout belongs to the reporter — the human tree, `--format json`'s JSONL,
//!    TAP. A single stray byte there corrupts a machine format for every consumer. Nothing in this
//!    module writes to stdout, and `proofs/progress/activity_test.lua` holds that to a proof.
//! 2. **Threshold-gated.** An activity that finishes fast prints *nothing at all* — not a start line,
//!    not a completion. Announcing a 40ms `echo` is worse than silence: it trains the reader to skim
//!    past the lines that matter. Only when a pause crosses [`THRESHOLD`] does it become visible, and
//!    then the start line is emitted retroactively.
//!
//! The retroactive start is what makes gating possible without a background timer: [`begin`] records
//! a start instant and prints nothing, and whoever next touches that activity (an `update`, or the
//! `finish`) decides — with the elapsed time in hand — whether it was worth mentioning.
//!
//! The cost is that a pause with no updates says nothing *until it ends*, which is exactly backwards
//! for the case we care about. So the pull path calls `update` per layer, and a reaper thread
//! ([`spawn_reaper`]) sweeps live activities every 250ms and announces any that have crossed the
//! threshold. That keeps the common case (fast, silent) allocation-light while still speaking during
//! a long silent wait.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use prova_core::progress::{Kind, Progress};

/// How long a pause must last before it is worth a line. Below this, silence.
///
/// 400ms is chosen to sit above the noise (a local `docker inspect`, a cached image check, a fast
/// `shell.run`) and below human "is this thing on?" — which starts around a second.
const THRESHOLD: Duration = Duration::from_millis(400);

/// How often the reaper looks for activities that have crossed the threshold.
const SWEEP: Duration = Duration::from_millis(250);

/// When to render activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Render when stderr is a terminal OR when explicitly piped somewhere a human/agent reads.
    /// In practice: on unless stderr is closed. Activity is plain text, so unlike a spinner there is
    /// no reason to suppress it just because the stream is not a tty.
    Auto,
    /// Always render.
    Always,
    /// Never render. The escape hatch for a caller that wants byte-identical stderr across runs.
    Never,
}

impl Mode {
    /// Parse a `--progress` value / `PROVA_PROGRESS` / manifest string.
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s {
            "auto" => Ok(Mode::Auto),
            "always" => Ok(Mode::Always),
            "never" | "none" | "off" => Ok(Mode::Never),
            other => Err(format!(
                "unknown progress mode {other:?} — expected auto, always, or never"
            )),
        }
    }
}

/// One activity the renderer is tracking.
struct Live {
    kind: Kind,
    subject: String,
    started: Instant,
    /// Whether a start line has been printed for it. Once true, the completion line must print too —
    /// an announced activity that never resolves is the exact thing this feature exists to prevent.
    announced: bool,
    /// The most recent detail from `update`, shown when the start line is finally emitted.
    detail: Option<String>,
}

/// The stderr renderer.
pub struct StderrProgress {
    live: Mutex<BTreeMap<u64, Live>>,
    next_id: AtomicU64,
}

impl StderrProgress {
    pub fn new() -> Arc<StderrProgress> {
        Arc::new(StderrProgress {
            live: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(0),
        })
    }

    /// Start the sweep thread that announces long-running activities *while* they are running.
    /// Without it, a silent 60s wait would only speak at the end — which is too late to be the
    /// "it isn't hung" signal.
    /// The reaper holds a `Weak`, so it exits on its own when the last sink handle drops — no
    /// lifecycle call for a caller to forget, and no thread outliving the run that spawned it. That
    /// matters for MCP mode, where the process is long-lived and a leaked sweeper per request would
    /// accumulate.
    pub fn spawn_reaper(self: &Arc<Self>) {
        let me: Weak<Self> = Arc::downgrade(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(SWEEP);
            let Some(me) = me.upgrade() else {
                return; // the run is over
            };
            let Ok(mut live) = me.live.lock() else {
                return;
            };
            for entry in live.values_mut() {
                if !entry.announced && entry.started.elapsed() >= THRESHOLD {
                    announce(entry);
                }
            }
        });
    }

    /// Find a live activity by (kind, subject) — the identity `Progress` gives us, since the trait
    /// deliberately passes values rather than a handle (keeping it object-safe and letting a null
    /// implementation cost nothing).
    fn find(live: &mut BTreeMap<u64, Live>, kind: Kind, subject: &str) -> Option<u64> {
        live.iter()
            .find(|(_, e)| e.kind == kind && e.subject == subject)
            .map(|(id, _)| *id)
    }
}

/// Emit the retroactive start line for an activity that has proven slow enough to matter.
fn announce(entry: &mut Live) {
    let detail = entry
        .detail
        .as_deref()
        .map(|d| format!(" ({d})"))
        .unwrap_or_default();
    let _ = writeln!(
        std::io::stderr(),
        "prova: {} {}{}…",
        entry.kind.verb(),
        entry.subject,
        detail
    );
    entry.announced = true;
}

impl Progress for StderrProgress {
    fn begin(&self, kind: Kind, subject: &str) {
        // Deliberately silent: gating happens later, with the elapsed time in hand.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut live) = self.live.lock() {
            live.insert(
                id,
                Live {
                    kind,
                    subject: subject.to_string(),
                    started: Instant::now(),
                    announced: false,
                    detail: None,
                },
            );
        }
    }

    fn update(&self, kind: Kind, subject: &str, detail: &str) {
        let Ok(mut live) = self.live.lock() else {
            return;
        };
        let Some(id) = Self::find(&mut live, kind, subject) else {
            return;
        };
        // `find` just returned the id, but progress is best-effort display: an entry that
        // somehow is not there is a no-op line, never a panic in the middle of someone's run.
        let Some(entry) = live.get_mut(&id) else {
            return;
        };
        entry.detail = Some(detail.to_string());
        if !entry.announced && entry.started.elapsed() >= THRESHOLD {
            announce(entry);
        }
    }

    fn finish(&self, kind: Kind, subject: &str, elapsed: Duration, note: Option<&str>) {
        let Ok(mut live) = self.live.lock() else {
            return;
        };
        let Some(id) = Self::find(&mut live, kind, subject) else {
            return;
        };
        let Some(entry) = live.remove(&id) else {
            return;
        };

        // The gate. A fast activity leaves no trace at all — not even a completion line, because a
        // completion with no start reads as a non-sequitur.
        if !entry.announced && elapsed < THRESHOLD {
            return;
        }
        if !entry.announced {
            // Crossed the threshold without the reaper catching it: say what happened, once, in a
            // single line rather than a start/done pair for something already over.
            let note = note.map(|n| format!(", {n}")).unwrap_or_default();
            let _ = writeln!(
                std::io::stderr(),
                "prova: {} {} — {:.1}s{}",
                kind.verb(),
                subject,
                elapsed.as_secs_f64(),
                note
            );
            return;
        }
        let note = note.map(|n| format!(", {n}")).unwrap_or_default();
        let _ = writeln!(
            std::io::stderr(),
            "prova: {} {} — done in {:.1}s{}",
            kind.verb(),
            subject,
            elapsed.as_secs_f64(),
            note
        );
    }
}

/// Build the sink for `mode`. `Never` yields the core's silent sink, so the whole facility costs an
/// `Arc` deref and a branch when it is off.
pub fn sink(mode: Mode) -> Arc<dyn Progress> {
    match mode {
        Mode::Never => prova_core::progress::null(),
        Mode::Auto | Mode::Always => {
            let p = StderrProgress::new();
            p.spawn_reaper();
            p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_the_documented_spellings() {
        assert_eq!(Mode::parse("auto").unwrap(), Mode::Auto);
        assert_eq!(Mode::parse("always").unwrap(), Mode::Always);
        assert_eq!(Mode::parse("never").unwrap(), Mode::Never);
        // Two spellings people reach for reflexively; accepting them costs nothing and a hard error
        // on `off` would be a papercut in exactly the moment someone is trying to quiet output.
        assert_eq!(Mode::parse("off").unwrap(), Mode::Never);
        assert_eq!(Mode::parse("none").unwrap(), Mode::Never);
        assert!(Mode::parse("loud").is_err());
    }

    // The gate, which is the difference between useful and noise: a fast activity must leave no
    // trace, so instrumenting a hot path stays free.
    #[test]
    fn a_fast_activity_is_never_announced() {
        let p = StderrProgress::new();
        p.begin(Kind::Command, "echo hi");
        p.finish(Kind::Command, "echo hi", Duration::from_millis(5), None);
        assert!(
            p.live.lock().unwrap().is_empty(),
            "the activity must be reaped, announced or not"
        );
    }

    // An announced activity must always resolve — an open "pulling…" that never completes is the
    // failure this whole feature exists to prevent, and would be worse than the silence it replaced.
    #[test]
    fn finishing_always_clears_the_live_entry() {
        let p = StderrProgress::new();
        p.begin(Kind::Pull, "postgres:16");
        p.update(Kind::Pull, "postgres:16", "2 layers");
        assert_eq!(p.live.lock().unwrap().len(), 1);
        p.finish(Kind::Pull, "postgres:16", Duration::from_secs(3), Some("5 layers"));
        assert!(p.live.lock().unwrap().is_empty());
    }

    // Concurrent provisioning is normal (parallel workers), so two activities of the same kind must
    // not be confused for one another.
    #[test]
    fn two_activities_are_tracked_independently() {
        let p = StderrProgress::new();
        p.begin(Kind::Pull, "postgres:16");
        p.begin(Kind::Pull, "redis:7");
        assert_eq!(p.live.lock().unwrap().len(), 2);
        p.finish(Kind::Pull, "redis:7", Duration::from_millis(10), None);
        let live = p.live.lock().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live.values().next().unwrap().subject, "postgres:16");
    }

    // A finish for something never begun (a renderer swapped mid-run, a double-finish) must be inert
    // rather than panicking — this runs on worker threads where a panic is a lost result.
    #[test]
    fn finishing_an_unknown_activity_is_inert() {
        let p = StderrProgress::new();
        p.finish(Kind::Build, "never-started", Duration::from_secs(9), None);
        p.update(Kind::Build, "never-started", "detail");
    }
}
