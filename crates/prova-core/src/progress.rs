//! Activity reporting — what is happening during a pause, so a run never just looks hung.
//!
//! A run can sit for tens of seconds with no output: a cold `postgres:16-alpine` pull, a container
//! readiness poll, a `cargo build` inside a fixture. Every one of those lives **below** the plugin
//! surface, inside prova's own kernel, where no plugin author can reach — so "add a print in the
//! plugin" cannot fix them (docs/plans/run-progress-feedback.md).
//!
//! # Why this is not an `Event`
//!
//! The obvious move — a `NodeProgress` variant on [`crate::Event`] — is wrong on three counts, and
//! the whole shape of this module follows from avoiding them:
//!
//! - `Event` is rendered to **stdout** (human tree, JSON, TAP) or a JUnit file. Activity on stdout
//!   corrupts `--format json` and `tap` for every consumer, agents included.
//! - `Event` is marshalled across a thread channel as *owned* data so parallel workers can report
//!   results. Transient chatter does not belong in a results channel.
//! - `Event` is the **durable** test-lifecycle record. Activity is **ephemeral**. Coupling the two
//!   collapses a seam the architecture keeps clean on purpose.
//!
//! So: activity is its own concern, on its own stream (**stderr**), with its own lifetime. It rides
//! [`RunConfig`](crate::RunConfig) rather than the reporter, and the two never meet.
//!
//! # The contract
//!
//! An [`Activity`] is a scope, not a message: [`Progress::start`] returns a handle, and finishing it
//! reports the elapsed time. Callers bracket a blocking region and say nothing about *when* to print
//! — that is the renderer's decision, which is what makes threshold gating possible (a pull that
//! takes 80ms should print nothing at all).
//!
//! Implementations must be cheap when nothing is listening: the default [`NullProgress`] does no
//! work and allocates nothing, so instrumenting a hot path costs an `Arc` deref and a branch.
//!
//! Implementations must also be **thread-safe and reentrant**: parallel workers provision
//! concurrently, so several activities can be live at once and finish out of order.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// What kind of pause this is. The renderer uses it for wording and, later, for whether a TTY
/// enrichment (a byte bar for a pull, a spinner for a poll) makes sense — Phase 2 territory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Pulling a container image — the dominant cause of a silent run.
    Pull,
    /// Building an image or a project.
    Build,
    /// Running an external command whose output is captured.
    Command,
    /// Polling something until it is ready (container readiness, a client connect-retry).
    Waiting,
    /// Fetching a remote source (a plugin's git checkout, an archetype).
    Fetch,
    /// Rendering an archetype.
    Render,
}

impl Kind {
    /// The verb a renderer leads with, present-continuous — these lines answer "what is it doing?".
    pub fn verb(self) -> &'static str {
        match self {
            Kind::Pull => "pulling",
            Kind::Build => "building",
            Kind::Command => "running",
            Kind::Waiting => "waiting for",
            Kind::Fetch => "fetching",
            Kind::Render => "rendering",
        }
    }
}

/// A live activity. Report completion by calling [`Activity::done`]; dropping without it is treated
/// as an abandoned activity and reported the same way, so a `?` early-return can never strand a
/// "still going" line on screen.
#[must_use = "an Activity reports nothing until it is finished or dropped"]
pub struct Activity {
    progress: Arc<dyn Progress>,
    kind: Kind,
    subject: String,
    started: Instant,
    finished: bool,
}

impl Activity {
    /// Finish successfully, reporting how long it took.
    pub fn done(mut self) {
        self.finished = true;
        self.progress
            .finish(self.kind, &self.subject, self.started.elapsed(), None);
    }

    /// Finish with a short outcome note (`"cached"`, `"3 layers"`, an error summary).
    pub fn done_with(mut self, note: impl Into<String>) {
        self.finished = true;
        let note = note.into();
        self.progress
            .finish(self.kind, &self.subject, self.started.elapsed(), Some(&note));
    }

    /// Report intermediate detail on a long activity (a pull's layer counts). Renderers are free to
    /// coalesce or ignore these; nothing may depend on one being shown.
    pub fn update(&self, detail: &str) {
        self.progress.update(self.kind, &self.subject, detail);
    }

    /// How long this activity has been running — for a caller that wants to threshold on its own.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        if !self.finished {
            // An early return (`?`) or a panic unwinding past the bracket. Reporting it as finished
            // keeps the renderer's bookkeeping balanced — an unbalanced start is exactly the "still
            // pulling…" line that never resolves.
            self.progress
                .finish(self.kind, &self.subject, self.started.elapsed(), None);
        }
    }
}

/// The sink for activity. Implemented by the CLI's stderr renderer; [`NullProgress`] elsewhere.
///
/// Every method takes `&self` and must be safe to call from multiple worker threads at once.
pub trait Progress: Send + Sync {
    /// A blocking region began. Implementations should NOT print here unconditionally — the whole
    /// point of the threshold is that a fast operation stays silent.
    fn begin(&self, kind: Kind, subject: &str);

    /// Intermediate detail on a running activity.
    fn update(&self, kind: Kind, subject: &str, detail: &str);

    /// The region ended, having taken `elapsed`.
    fn finish(&self, kind: Kind, subject: &str, elapsed: Duration, note: Option<&str>);
}

/// The default: reports nothing, allocates nothing. What library consumers and every test get unless
/// a renderer is installed.
pub struct NullProgress;

impl Progress for NullProgress {
    fn begin(&self, _kind: Kind, _subject: &str) {}
    fn update(&self, _kind: Kind, _subject: &str, _detail: &str) {}
    fn finish(&self, _kind: Kind, _subject: &str, _elapsed: Duration, _note: Option<&str>) {}
}

/// A silent sink, ready to hand. The type alias keeps call sites from having to spell the trait
/// object out just to opt out of reporting.
pub type NullProgressArc = Arc<dyn Progress>;

/// A silent sink — what a library consumer or a test wants when activity is not what it is testing.
pub fn null() -> NullProgressArc {
    Arc::new(NullProgress)
}

/// Open an activity scope on `progress`.
///
/// A free function rather than a trait method so `Progress` stays object-safe with a minimal surface
/// (`Activity` needs to hold the `Arc`, which a `&self` method cannot hand out).
pub fn start(progress: &Arc<dyn Progress>, kind: Kind, subject: impl Into<String>) -> Activity {
    let subject = subject.into();
    progress.begin(kind, &subject);
    Activity {
        progress: Arc::clone(progress),
        kind,
        subject,
        started: Instant::now(),
        finished: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<String>>,
    }

    impl Progress for Recorder {
        fn begin(&self, kind: Kind, subject: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("begin {} {subject}", kind.verb()));
        }
        fn update(&self, _kind: Kind, subject: &str, detail: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("update {subject} {detail}"));
        }
        fn finish(&self, _kind: Kind, subject: &str, _elapsed: Duration, note: Option<&str>) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("finish {subject} {}", note.unwrap_or("-")));
        }
    }

    fn recorder() -> (Arc<dyn Progress>, Arc<Recorder>) {
        let rec = Arc::new(Recorder::default());
        (Arc::clone(&rec) as Arc<dyn Progress>, rec)
    }

    #[test]
    fn an_activity_brackets_begin_and_finish() {
        let (progress, rec) = recorder();
        let a = start(&progress, Kind::Pull, "postgres:16-alpine");
        a.update("2 of 5 layers");
        a.done_with("3 layers");
        assert_eq!(
            *rec.calls.lock().unwrap(),
            vec![
                "begin pulling postgres:16-alpine",
                "update postgres:16-alpine 2 of 5 layers",
                "finish postgres:16-alpine 3 layers",
            ]
        );
    }

    // The `?`-early-return case. An unbalanced begin is what leaves a "still pulling…" line on
    // screen forever, so the Drop impl closes it — this is the guard for that, not a nicety.
    #[test]
    fn dropping_an_activity_still_finishes_it() {
        let (progress, rec) = recorder();
        {
            let _a = start(&progress, Kind::Fetch, "prova-redis");
        }
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "begin + finish, got {calls:?}");
        assert!(calls[1].starts_with("finish prova-redis"));
    }

    #[test]
    fn the_null_sink_is_silent_and_safe() {
        let progress: Arc<dyn Progress> = Arc::new(NullProgress);
        let a = start(&progress, Kind::Waiting, "postgres");
        a.update("attempt 3");
        a.done();
    }
}
