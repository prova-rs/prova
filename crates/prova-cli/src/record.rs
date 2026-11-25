//! The run record — what a run actually executed, and what it did not.
//!
//! `0 failed` is the last thing an *honest* agent can say that is true and worthless. It is true of
//! a run in which every proof skipped for want of a docker daemon, true of a run whose selection
//! matched three tests out of four hundred, and true of a run that collected nothing at all. The
//! failure count answers "did anything go red"; nobody was asking that. They were asking "is this
//! covered", and the negative space — the skipped, the deselected — is where that answer lives.
//!
//! So each run writes down what it did. Counts, and then the individual paths behind the two counts
//! that mean *no evidence was produced*. [`attest`] reads it back and answers the question about one
//! obligation: not "did the suite pass" but "did the proof for THIS claim actually run".
//!
//! ## Deliberately not signed
//!
//! The threat model is a careless agent, not a malicious one. An agent that would forge a record
//! would equally write a falsifier that mutates nothing, and a signature buys ceremony and key
//! management rather than truth. The honest ceiling here is "an agent cannot be wrong by accident".
//!
//! ## Where it lives
//!
//! `<home>/.prova/var/last-run.json` on every run — prova's own self-ignoring state directory, so it
//! costs the user no ignore-file edits and never lands in a tracked tree. `--record <path>` *also*
//! emits it wherever asked for, for CI to keep as an artifact or a bot to post; the var/ copy is for the
//! next command, the emitted one is for a human.
//!
//! ## Types
//!
//! The canonical record types live in `prova_core::ledger`; the CLI is one renderer over that
//! ledger, not its owner. This module keeps the CLI-side policy: where the record lives, how to read
//! and write it, and the conversions from engine types into record rows.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::home::Home;
use crate::var;

/// The record's basename in `var/`. No leading dot: it already sits in a hidden, self-ignoring
/// directory, and a second layer of hiding only makes it harder to read while debugging.
pub const LAST_RUN: &str = "last-run.json";

/// Where a run that was not green keeps its own record, by run id (`runs/<run-id>.json`): the
/// rerun that follows a red overwrites `last-run.json`, and with it the only copy of which case
/// failed and what it said. A flake row names its `failed_run`; this is where that run is read.
pub const KEPT_RUNS: &str = "runs";

/// How many not-green records `runs/` keeps, newest first: a count, never an age, so a quiet
/// package keeps its last reds however long ago they were.
pub const KEPT_RUNS_MAX: usize = 32;

// The canonical run-record types live in the core ledger so embeddings can read the same file.
pub use prova_core::ledger::{
    attest, Attested, Counts, DeputedRow, Executed, MeasurementRow, Record, ReminderEntry,
    ReportRow, Skipped,
};

/// Convert the engine's evaluated reminders into record rows.
pub fn reminder_entries(outcomes: &[prova_core::ReminderOutcome]) -> Vec<ReminderEntry> {
    use prova_core::ReminderState;
    outcomes
        .iter()
        .map(|o| {
            let (state, why) = match &o.state {
                ReminderState::Watching => ("watching", None),
                ReminderState::Due { why } => ("due", why.clone()),
                ReminderState::Unevaluated { reason } => ("unevaluated", Some(reason.clone())),
            };
            ReminderEntry {
                name: o.name.clone(),
                state: state.to_string(),
                why,
                message: o.message.clone(),
                tags: o.tags.clone(),
                file: o.file.clone(),
                line: o.line,
            }
        })
        .collect()
}

/// Convert the engine's ingested cases into record rows.
pub fn deputed_rows(cases: &[prova_core::DeputedCase]) -> Vec<DeputedRow> {
    cases
        .iter()
        .map(|c| DeputedRow {
            verifier: c.verifier.clone(),
            suite: c.suite.clone(),
            name: c.name.clone(),
            outcome: c.outcome.clone(),
            message: c.message.clone(),
            time_ms: c.time_ms,
            file: c.file.clone(),
            reused_from: c.reused_from.clone(),
            flaky_from: c.flaky_from.clone(),
            output: c.output.clone(),
        })
        .collect()
}

/// Convert the engine's recorded measurements into record rows.
pub fn measurement_rows(measurements: &[prova_core::Measurement]) -> Vec<MeasurementRow> {
    measurements
        .iter()
        .map(|m| MeasurementRow {
            name: m.name.clone(),
            value: m.value,
            // The stable string form lives on the core type, shared with the baseline file.
            direction: m.direction.as_str().to_string(),
            set: m.set.clone(),
        })
        .collect()
}

/// Convert the engine's published reports into record rows.
///
/// Paths render through `emit_path` for the same reason every path-producing API does: the record
/// is read by agents and pasted into shells, and a Windows `\` in JSON is an escape nobody meant.
pub fn report_rows(reports: &[prova_core::Report]) -> Vec<ReportRow> {
    reports
        .iter()
        .map(|r| ReportRow {
            name: r.name.clone(),
            summary: r.summary.clone(),
            explains: r.explains.clone(),
            forms: r
                .forms
                .iter()
                .map(|f| (f.kind.clone(), prova_core::emit_path(&f.path)))
                .collect(),
        })
        .collect()
}

/// Which build produced a record.
///
/// A content hash of the executable would be the obvious answer and is the wrong one: prova's own
/// binary is hundreds of megabytes unoptimized, and hashing it on *every* run would put a visible
/// tax on the ordinary path — which is exactly how machinery gets switched off. Version plus the
/// executable's size and mtime distinguishes every build that a developer or CI will actually
/// produce, at the cost of one `stat`.
pub fn binary_fingerprint() -> String {
    let mut hasher = <Sha256 as Digest>::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    if let Ok(exe) = prova_core::current_exe() {
        hasher.update(exe.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(&exe) {
            hasher.update(meta.len().to_le_bytes());
            if let Ok(modified) = meta.modified() {
                if let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) {
                    hasher.update(since.as_secs().to_le_bytes());
                }
            }
        }
    }
    hex::encode(hasher.finalize())[..12].to_string()
}

/// A leaf's path, qualified by the file it was declared in — the record's key form.
///
/// One definition, in core ([`prova_core::qualify_leaf_path`]), because the deselected are
/// qualified there (they emit no event to carry a file) and the executed and skipped are qualified
/// here. Two spellings of one address would put a proof in the record under a name `attest` could
/// not find.
pub fn qualified(path: &str, file: Option<&str>) -> String {
    prova_core::qualify_leaf_path(path, file.map(Path::new))
}

/// The record path for a package. Resolves without creating anything, so a read on a package that
/// has never run leaves no directory behind.
pub fn path(home: &Home) -> PathBuf {
    var::path(home).join(LAST_RUN)
}

/// Read the last run's record, if one exists and parses.
pub fn load(home: &Home) -> Option<prova_core::ledger::Record> {
    prova_core::ledger::read_record(&path(home)).ok()
}

/// Write the record into `var/` (materializing it), and to `also` when `--record` asked for a copy.
///
/// Best-effort by construction: a run's verdict must never depend on whether its record could be
/// written. A package with no manifest home has nowhere durable to put one and quietly records
/// nothing, exactly as `--last-failed` does.
pub fn store(home: &Option<Home>, record: &Record, also: Option<&Path>) {
    let Ok(text) = serde_json::to_string_pretty(record) else {
        return;
    };
    if let Some(home) = home.as_ref() {
        if let Ok(dir) = var::dir(home) {
            let _ = std::fs::write(dir.join(LAST_RUN), &text);
            if record.verdict != "green" {
                keep(&dir.join(KEPT_RUNS), &record.run_id, &text);
            }
        }
    }
    if let Some(dest) = also {
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(dest, &text) {
            // The var/ copy is a byproduct; an explicitly requested one is an instruction, and
            // silently not honoring it would be its own small lie.
            eprintln!("prova: --record: could not write {}: {e}", dest.display());
        }
    }
}

/// Keep a not-green record as `<runs>/<run_id>.json`, then prune to the newest [`KEPT_RUNS_MAX`].
/// Run ids lead with their start in epoch milliseconds (`run-<ms>-<pid>`), so name order is age
/// order. Best effort, as `store` is.
fn keep(runs: &Path, run_id: &str, text: &str) {
    if run_id.is_empty() || run_id.contains(['/', '\\']) || std::fs::create_dir_all(runs).is_err() {
        return;
    }
    let _ = std::fs::write(runs.join(format!("{run_id}.json")), text);
    let Ok(entries) = std::fs::read_dir(runs) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("run-") && n.ends_with(".json"))
        .collect();
    names.sort_by(|a, b| b.cmp(a));
    for stale in names.iter().skip(KEPT_RUNS_MAX) {
        let _ = std::fs::remove_file(runs.join(stale));
    }
}

/// The most bytes `run show` reads of one record: a record is a run's account, never a log, so one this
/// large is not a record and is refused by name rather than read whole.
pub const SHOW_BYTES: u64 = 16 * 1024 * 1024;

/// The shape `run show --format json` prints. A reader refuses a higher one by name, and skips a key it
/// doesn't know (rule 8): keys are added within schema 1, never renamed or retyped.
pub const SHOW_SCHEMA: u32 = 1;

/// Whether `id` is a prova run id, `run-<ms>-<pid>`: never a path, so `run show` reads one record and
/// nothing else.
pub fn is_run_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("run-") else {
        return false;
    };
    let mut parts = rest.split('-');
    let digits =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// Where `id`'s record is, under `var` (this package's resolved state directory): kept by id when the
/// run wasn't green, else `last-run.json` while it is still that run's. `None` when neither holds it.
pub fn find_run(var: &Path, id: &str) -> Option<PathBuf> {
    let kept = var.join(KEPT_RUNS).join(format!("{id}.json"));
    if kept.is_file() {
        return Some(kept);
    }
    let last = var.join(LAST_RUN);
    let text = read_bounded(&last).ok()?;
    let run_id = serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("run_id")?
        .as_str()?
        .to_owned();
    (run_id == id).then_some(last)
}

/// `path` read whole, refused past [`SHOW_BYTES`].
fn read_bounded(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut text = String::new();
    file.take(SHOW_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if text.len() as u64 > SHOW_BYTES {
        return Err(format!(
            "{} is larger than {SHOW_BYTES} bytes, so it isn't read as a record",
            path.display()
        ));
    }
    Ok(text)
}

/// A record as `run show` prints it: its id, verdict, the tree it ran on (prova's fingerprint, the one
/// `prova tree --rev` prints) and every row, its own and those deputed to a verifier, each with its
/// outcome. `commit` is null: a record names the bytes it ran on, never a commit id.
pub fn shown(record: &Record, read: &Path) -> serde_json::Value {
    let mut rows: Vec<serde_json::Value> = record
        .executed
        .iter()
        .map(|(name, outcome)| {
            let failure = record.failures.get(name);
            serde_json::json!({
                "name": name,
                "outcome": serde_json::to_value(outcome).unwrap_or(serde_json::Value::Null),
                "file": null,
                "line": failure.and_then(|f| f.line),
                "message": failure.map(|f| f.message.as_str()),
                "verifier": null,
            })
        })
        .collect();
    rows.extend(record.deputed.iter().map(|d| {
        serde_json::json!({
            "name": d.name,
            "outcome": d.outcome,
            "file": d.file,
            "line": null,
            "message": d.message,
            "verifier": d.verifier,
            "suite": d.suite,
        })
    }));
    serde_json::json!({
        "schema": SHOW_SCHEMA,
        "id": record.run_id,
        "verdict": record.verdict,
        "tree": record.tree,
        "commit": null,
        "prova": record.version,
        "record": prova_core::emit_path(read),
        "summary": record.summary,
        "rows": rows,
    })
}

/// `prova run show <run-id> --format json [--manifest PATH]`: one run's record, read from this package's
/// state directory as prova resolves it (`PROVA_VAR_DIR` too, E m/18306), printed in [`shown`]'s shape. A
/// run prova doesn't hold is refused naming the id and the directory read, with nothing on stdout.
pub(crate) fn show_subcommand(args: Vec<String>) -> std::process::ExitCode {
    let (mut id, mut format, mut manifest_path) = (None, None, None);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let slot = match arg.as_str() {
            "-h" | "--help" => {
                println!("usage: prova run show <run-id> --format json [--manifest PATH]");
                return std::process::ExitCode::SUCCESS;
            }
            "--format" => &mut format,
            "--manifest" => &mut manifest_path,
            other if id.is_none() && !other.starts_with('-') => {
                id = Some(other.to_owned());
                continue;
            }
            other => {
                eprintln!("prova run show: unexpected argument {other:?}");
                return crate::exit::config();
            }
        };
        match it.next() {
            Some(v) => *slot = Some(v),
            None => {
                eprintln!("prova run show: {arg} needs a value");
                return crate::exit::config();
            }
        }
    }
    let Some(id) = id else {
        eprintln!("prova run show: name the run, `prova run show <run-id> --format json`");
        return crate::exit::config();
    };
    if format.as_deref().is_some_and(|f| f != "json") {
        eprintln!("prova run show: --format json is the one format");
        return crate::exit::config();
    }
    if !is_run_id(&id) {
        eprintln!("prova run show: {id:?} isn't a prova run id (run-<ms>-<pid>)");
        return crate::exit::config();
    }
    if let Err(why) = var::check_env() {
        eprintln!("prova: {why}");
        return crate::exit::config();
    }
    let home = match crate::cmd_topo::resolve_home(manifest_path.as_deref()) {
        Ok(h) => h,
        Err(code) => return code,
    };
    let dir = var::path(&home);
    let Some(path) = find_run(&dir, &id) else {
        eprintln!(
            "prova run show: no run {id} in {}",
            prova_core::emit_path(&dir)
        );
        return crate::exit::config();
    };
    let record = match read_bounded(&path).and_then(|text| {
        serde_json::from_str::<Record>(&text)
            .map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }) {
        Ok(r) => r,
        Err(why) => {
            eprintln!("prova run show: {why}");
            return crate::exit::config();
        }
    };
    println!("{}", shown(&record, &path));
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A kept not-green record, by run id (`runs/<run-id>.json`).
    fn kept(home: &Home, run_id: &str) -> Option<prova_core::ledger::Record> {
        prova_core::ledger::read_record(
            &var::path(home)
                .join(KEPT_RUNS)
                .join(format!("{run_id}.json")),
        )
        .ok()
    }

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("prova-record-ut-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home_in(dir: &Path) -> Home {
        Home {
            dir: dir.to_path_buf(),
            manifest: dir.join("prova.toml"),
        }
    }

    fn minimal_record() -> Record {
        Record {
            schema: 1,
            version: "0.0.0-test".into(),
            binary: "test".into(),
            run_id: "run-test".into(),
            tree: Some("tree-test".into()),
            reused_from: BTreeMap::new(),
            reused_inputs: BTreeMap::new(),
            selection: vec!["engine".into()],
            duration_ms: 7,
            summary: Counts {
                passed: 2,
                ..Counts::default()
            },
            executed: BTreeMap::from([("f › t".to_string(), Executed::Passed)]),
            failures: BTreeMap::new(),
            skipped: vec![],
            deselected: vec!["f › other".into()],
            measurements: vec![],
            attached: vec![],
            dependency_overrides: Vec::new(),
            reports: Vec::new(),
            reminders: vec![],
            deputed: vec![],
            deputed_narrowed: false,
            deputed_output_unkept: 0,
            build_dir: None,
            verdict: "green".into(),
            flakes: vec![],
        }
    }

    /// The record's key form: the declaring file's stem prefixes the path, unless the path
    /// already leads with it — the rule that keeps executed/skipped (qualified here) and
    /// deselected (qualified in core) under ONE spelling per address.
    #[test]
    fn qualified_prefixes_the_file_stem_exactly_once() {
        assert_eq!(qualified("a › b", Some("proofs/x.lua")), "x › a › b");
        assert_eq!(qualified("x › a", Some("proofs/x.lua")), "x › a");
        assert_eq!(qualified("a › b", None), "a › b");
    }

    /// Each mapper carries the engine's fields through by name; the reminder mapper also owns
    /// the state→string spelling, which the record file then freezes.
    #[test]
    fn row_mappers_carry_engine_fields_and_spell_reminder_states() {
        use prova_core::{ReminderOutcome, ReminderState};
        let spell = |state: ReminderState| {
            let rows = reminder_entries(&[ReminderOutcome {
                name: "n".into(),
                message: "do the thing".into(),
                tags: vec!["ops".into()],
                file: Some("r.lua".into()),
                line: Some(3),
                state,
            }]);
            (rows[0].state.clone(), rows[0].why.clone())
        };
        assert_eq!(spell(ReminderState::Watching), ("watching".into(), None));
        assert_eq!(
            spell(ReminderState::Due {
                why: Some("date passed".into())
            }),
            ("due".into(), Some("date passed".into()))
        );
        assert_eq!(
            spell(ReminderState::Unevaluated {
                reason: "no clock".into()
            }),
            ("unevaluated".into(), Some("no clock".into()))
        );

        let deputed = deputed_rows(&[prova_core::DeputedCase {
            verifier: "junit".into(),
            suite: "SuiteA".into(),
            name: "case_1".into(),
            outcome: "failed".into(),
            message: Some("boom".into()),
            time_ms: Some(12),
            file: "target/junit.xml".into(),
            reused_from: None,
            flaky_from: None,
            output: None,
        }]);
        assert_eq!(deputed[0].verifier, "junit");
        assert_eq!(deputed[0].outcome, "failed");
        assert_eq!(deputed[0].time_ms, Some(12));
        assert_eq!(deputed[0].output, None, "a case with no output keeps none");
        let mut red = prova_core::DeputedCase {
            verifier: "junit".into(),
            suite: "s".into(),
            name: "red".into(),
            outcome: "failed".into(),
            message: Some("boom".into()),
            time_ms: None,
            file: "j.xml".into(),
            reused_from: None,
            flaky_from: None,
            output: Some("thread 'red' panicked: the node stopped".into()),
        };
        let row = deputed_rows(std::slice::from_ref(&red)).remove(0);
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(
            json["output"], "thread 'red' panicked: the node stopped",
            "E m/21943: the record carries it"
        );
        red.output = None;
        let json = serde_json::to_value(deputed_rows(&[red]).remove(0)).unwrap();
        assert!(
            json.get("output").is_none(),
            "an older reader's shape when there is none"
        );

        let measured = measurement_rows(&[prova_core::Measurement {
            name: "rust.coverage.unit".into(),
            value: 64.9,
            direction: prova_core::Direction::HigherIsBetter,
            set: "quality".into(),
        }]);
        assert_eq!(measured[0].direction, "higher_is_better");
        assert_eq!(measured[0].set, "quality");
    }

    /// The full lifecycle in a tempdir: store materializes var/, load reads the same record
    /// back, a corrupt file reads as None (never an error), and an explicit `--record` copy
    /// lands even with no home to put the var/ copy in.
    #[test]
    fn store_then_load_round_trips_and_corruption_reads_as_none() {
        let dir = tempdir("roundtrip");
        let home = home_in(&dir);

        assert!(load(&home).is_none(), "a never-run package has no record");

        let record = minimal_record();
        store(&Some(home.clone()), &record, None);
        let back = load(&home).expect("stored record loads");
        assert_eq!(back.summary.passed, 2);
        assert_eq!(back.deselected, vec!["f › other".to_string()]);
        assert!(matches!(back.executed["f › t"], Executed::Passed));

        std::fs::write(path(&home), "{ not json").unwrap();
        assert!(load(&home).is_none(), "a corrupt record loads as None");

        let also = dir.join("artifacts").join("run.json");
        store(&None, &record, Some(&also));
        assert!(also.is_file(), "--record writes even with no package home");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rerun overwrites `last-run.json`; the red it reran stays readable by its run id, a green
    /// run keeps nothing, and `runs/` holds only the newest KEPT_RUNS_MAX.
    #[test]
    fn a_red_run_outlives_the_rerun_that_follows_it() {
        let dir = tempdir("kept");
        let home = home_in(&dir);
        let mut red = minimal_record();
        red.run_id = "run-1000000000001-1".into();
        red.verdict = "red".into();
        red.summary.failed = 1;
        store(&Some(home.clone()), &red, None);
        let mut green = minimal_record();
        green.run_id = "run-1000000000002-1".into();
        store(&Some(home.clone()), &green, None);
        assert_eq!(
            load(&home).expect("last").run_id,
            green.run_id,
            "the rerun is the last run"
        );
        assert_eq!(
            kept(&home, &red.run_id)
                .expect("the red is kept")
                .summary
                .failed,
            1
        );
        assert!(
            kept(&home, &green.run_id).is_none(),
            "a green run keeps nothing"
        );

        for n in 0..KEPT_RUNS_MAX + 3 {
            let mut r = red.clone();
            r.run_id = format!("run-{}-1", 2000000000000u64 + n as u64);
            store(&Some(home.clone()), &r, None);
        }
        let held = std::fs::read_dir(var::path(&home).join(KEPT_RUNS))
            .expect("runs/")
            .count();
        assert_eq!(held, KEPT_RUNS_MAX, "pruned to the newest");
        assert!(kept(&home, &red.run_id).is_none(), "the oldest went first");
        assert!(kept(
            &home,
            &format!("run-{}-1", 2000000000000u64 + KEPT_RUNS_MAX as u64 + 2)
        )
        .is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `run show` finds a red run kept by id, and a green one while `last-run.json` is still its own.
    #[test]
    fn show_finds_a_kept_run_and_the_last_green_one_by_id() {
        let dir = tempdir("show-find");
        let home = home_in(&dir);
        let mut red = minimal_record();
        red.run_id = "run-100-1".into();
        red.verdict = "red".into();
        store(&Some(home.clone()), &red, None);
        let mut green = minimal_record();
        green.run_id = "run-200-1".into();
        green.verdict = "green".into();
        store(&Some(home.clone()), &green, None);
        let var = var::path(&home);
        assert_eq!(
            find_run(&var, "run-100-1"),
            Some(var.join(KEPT_RUNS).join("run-100-1.json"))
        );
        assert_eq!(find_run(&var, "run-200-1"), Some(var.join(LAST_RUN)));
        assert_eq!(find_run(&var, "run-300-1"), None);
        assert!(
            is_run_id("run-100-1")
                && !is_run_id("run-1")
                && !is_run_id("../run-1-1")
                && !is_run_id("run-1-1-1")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shape carries every row with its outcome, own and deputed, and the tree, under schema 1.
    #[test]
    fn show_carries_the_rows_and_the_tree_under_its_schema() {
        let mut r = minimal_record();
        r.run_id = "run-100-1".into();
        r.tree = Some("git:abc".into());
        r.executed.insert("w › red".into(), Executed::Failed);
        r.deputed.push(DeputedRow {
            verifier: "junit".into(),
            suite: "s".into(),
            name: "mod::case".into(),
            outcome: "failed".into(),
            message: None,
            time_ms: None,
            file: "target/junit.xml".into(),
            reused_from: None,
            flaky_from: None,
            output: None,
        });
        let v = shown(&r, Path::new("/v/runs/run-100-1.json"));
        assert_eq!(v["schema"], 1);
        assert_eq!(
            (v["id"].as_str(), v["tree"].as_str()),
            (Some("run-100-1"), Some("git:abc"))
        );
        let rows = v["rows"].as_array().unwrap();
        let outcome = |n: &str| {
            rows.iter()
                .find(|r| r["name"] == n)
                .map(|r| r["outcome"].clone())
        };
        assert_eq!(outcome("w › red"), Some(serde_json::json!("failed")));
        assert_eq!(outcome("mod::case"), Some(serde_json::json!("failed")));
        assert!(v["commit"].is_null());
    }
}
