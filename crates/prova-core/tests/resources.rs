use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use prova_core::{run_path_with, NullReporter, RunConfig};

/// Run a testdata file at `concurrency`, returning its summary and the ordered event log the file
/// builds by calling the injected `record(event)` global.
///
/// What's under test is *overlap* — whether two holders of a resource may be inside their bodies at
/// the same time — not speed. Inferring that from wall-clock duration needs a threshold calibrated
/// to the machine, and a loaded runner blows straight through it: a macOS CI runner took 123ms for
/// ~40ms of work, which is longer than even the serialized case, so the thresholds both failed the
/// shared test and made the exclusive one pass vacuously. Event order answers the same question
/// exactly, at any speed.
fn run(file: &str, concurrency: usize) -> (prova_core::Summary, Vec<String>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/{file}"));

    // Each run is its own project, so its tokens lock under that project's `.prova/var/locks` and
    // nowhere else. Without one, a bare run's tokens are MACHINE-wide (locks::lock_dir), so these
    // files' "db" and port 8080 contended with each other's processes and with any prova on the
    // host: resource_remode's exclusive "db" writer, queued between this file's two shared
    // readers, held the second out while the first sat in its barrier, and the barrier timed out
    // at 20 s under the full suite (E, 2026-11-21; green alone). What's under test is how ONE run
    // schedules its own holders, so the lock scope is the run's.
    let project = Scratch::new(&format!("prova-resources-{}", file.trim_end_matches(".lua")));

    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = Arc::clone(&events);
    let config = RunConfig::new(concurrency).with_project(&project.0).with_module(move |lua| {
        let sink = Arc::clone(&sink);
        let record = lua.create_function(move |_, event: String| {
            sink.lock().expect("event log").push(event);
            Ok(())
        })?;
        lua.globals().set("record", record)?;
        Ok(())
    });

    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &config).expect("run");
    let events = events.lock().expect("event log").clone();
    (summary, events)
}

/// A directory of this process's own, removed when dropped, a panic included (L m/22011).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// The positions of `enter <name>` and `exit <name>` in the log: the span over which that test held
/// the resource.
fn interval(events: &[String], name: &str) -> (usize, usize) {
    let find = |event: String| {
        events
            .iter()
            .position(|e| *e == event)
            .unwrap_or_else(|| panic!("{event:?} missing from event log {events:?}"))
    };
    (find(format!("enter {name}")), find(format!("exit {name}")))
}

/// A run's tokens are its own project's: an exclusive hold on `db` in the MACHINE-wide lock
/// directory, as another process's bare run would take it, doesn't hold this run's shared readers
/// out, so their rendezvous completes. Red when the harness runs without a project (the 20 s
/// barrier timeout the full suite hit).
#[test]
fn a_runs_tokens_are_its_projects_and_a_machine_hold_never_reaches_them() {
    use std::os::fd::AsRawFd;
    // The machine-wide directory moved to one of this row's own (scratch::BASE_ENV), so the hold
    // below is a stand-in for another process's and never the host's shared state (L m/22011).
    // nextest runs each row in its own process, so the variable is this row's alone.
    let machine = Scratch::new("prova-resources-machine");
    // SAFETY: set before any thread of this process reads the environment.
    unsafe { std::env::set_var(prova_core::scratch::BASE_ENV, &machine.0) };
    let path = prova_core::locks::lock_path("db", false, None).expect("db's machine-wide lock file");
    assert!(path.starts_with(&machine.0), "the stand-in machine directory: {}", path.display());
    std::fs::create_dir_all(path.parent().expect("its directory")).expect("the lock directory");
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .expect("the lock file");
    // SAFETY: flock on an open descriptor we own; the hold ends when `held` drops.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0, "held");
    let (summary, events) = run("resource_retired_words.lua", 8);
    drop(held);
    assert_eq!(summary.passed, 4, "the readers met though another process held db: {events:?}");
    assert_overlap(&events, "ra", "rb", true);
}

/// Two exclusive holders of the same token must serialize even with concurrency headroom: the
/// writer↔writer conflict forces them one-at-a-time, so their spans are disjoint.
#[test]
fn exclusive_resource_serializes_under_concurrency() {
    let (summary, events) = run("resource_exclusive.lua", 8);
    assert_eq!(summary.passed, 2, "both pass");

    let (enter_a, exit_a) = interval(&events, "a");
    let (enter_b, exit_b) = interval(&events, "b");
    assert!(
        exit_a < enter_b || exit_b < enter_a,
        "expected exclusive holders to serialize (disjoint spans), got {events:?}"
    );
}

/// Two shared holders of the same token may run concurrently (reader ∥ reader): one enters while the
/// other is still inside, so their spans interleave.
#[test]
fn shared_resource_runs_concurrently() {
    let (summary, events) = run("resource_shared.lua", 8);
    assert_eq!(summary.passed, 2, "both pass");

    let (enter_a, exit_a) = interval(&events, "a");
    let (enter_b, exit_b) = interval(&events, "b");
    assert!(
        enter_a < exit_b && enter_b < exit_a,
        "expected shared readers to overlap (interleaved spans), got {events:?}"
    );
}

/// Assert the two spans overlap (concurrent) or are disjoint (serialized), naming which was expected.
fn assert_overlap(events: &[String], a: &str, b: &str, overlapping: bool) {
    let (enter_a, exit_a) = interval(events, a);
    let (enter_b, exit_b) = interval(events, b);
    let overlaps = enter_a < exit_b && enter_b < exit_a;
    assert_eq!(
        overlaps, overlapping,
        "expected {a}/{b} to {}, got {events:?}",
        if overlapping {
            "overlap (reader ∥ reader)"
        } else {
            "serialize (writer ⊥ writer)"
        }
    );
}

/// `reads`/`writes` are MODES over a token, so either can re-mode a ref the other made: two holders
/// of `prova.reads(prova.port(8080))` overlap, where `resource_exclusive.lua`'s writers on that same
/// port serialize. And `prova.writes` on a bare name serializes like the bare string it wraps.
#[test]
fn modes_are_independent_of_how_the_token_was_made() {
    let (summary, events) = run("resource_remode.lua", 8);
    assert_eq!(summary.passed, 4, "all four pass");

    assert_overlap(&events, "ra", "rb", true);
    assert_overlap(&events, "wa", "wb", false);
}

/// The retired spellings keep their successors' exact scheduling semantics, not just their arity:
/// `prova.shared` still overlaps like `reads`, `prova.resource` still serializes like `writes`. They
/// are hidden from `prova.help` (see `help::deprecated_entries_are_hidden_from_help`) but a suite
/// written before the rename must keep running — that promise is what this pins.
#[test]
fn retired_resource_words_keep_their_semantics() {
    let (summary, events) = run("resource_retired_words.lua", 8);
    assert_eq!(summary.passed, 4, "all four pass");

    assert_overlap(&events, "ra", "rb", true);
    assert_overlap(&events, "wa", "wb", false);
}
