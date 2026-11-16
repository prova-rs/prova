//! End-to-end (hermetic: no docker, no network): a red run's record keeps each red leaf's own words,
//! so a red can be read once the console is gone (E m/14546: Herald and Horai read the record, never
//! the scrollback). Each record is read back the consumer's way, through `ledger::read_record`.

use std::path::Path;
use std::process::Command;

use prova_core::ledger::{
    read_record, Executed, Record, CAUSE_INFRA, FAILURE_CUT_MARK, FAILURE_MESSAGE_AT_MOST,
};

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A scratch tree, removed on drop even when an assertion fails mid-test.
struct Tmp(std::path::PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let dir =
            std::env::temp_dir().join(format!("prova-red-words-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// A package whose one proof file is `proof`, run once with `--record`; the record as read back.
fn run(tmp: &Tmp, envs: &[(&str, &str)]) -> (std::process::Output, Record) {
    let out_path = tmp.0.join("record.json");
    let out = Command::new(env!("CARGO_BIN_EXE_prova"))
        .current_dir(&tmp.0)
        .args(["--record", out_path.to_str().unwrap()])
        .envs(envs.iter().copied())
        .env("XDG_CACHE_HOME", tmp.0.join(".x/cache"))
        .env("XDG_DATA_HOME", tmp.0.join(".x/data"))
        .env("XDG_CONFIG_HOME", tmp.0.join(".x/config"))
        .env_remove("PROVA_PROFILE")
        .env_remove("GITHUB_ACTIONS")
        .output()
        .expect("run prova");
    let record = read_record(&out_path).unwrap_or_else(|e| panic!("{e}: {}", said(&out)));
    (out, record)
}

fn package(tmp: &Tmp, proof: &str) {
    write(&tmp.0.join("prova.toml"), "[run]\nproofs = [\"tests\"]\n");
    write(&tmp.0.join("tests/one_test.lua"), proof);
}

fn said(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The one leaf the record says ended as `outcome`, by its record key.
fn leaf(record: &Record, outcome: Executed) -> String {
    let keys: Vec<&String> = record
        .executed
        .iter()
        .filter(|(_, o)| **o == outcome)
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys.len(), 1, "one {outcome:?} leaf: {:?}", record.executed);
    keys[0].clone()
}

#[test]
fn a_failing_leaf_writes_its_own_words_and_line_into_the_record() {
    let tmp = Tmp::new("failed");
    package(&tmp, "prova.test(\"one is two\", function(t)\n  t:expect(1):equals(2)\nend)\n\nprova.test(\"fine\", function(t) t:expect(1):equals(1) end)\n");
    let (out, record) = run(&tmp, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let key = leaf(&record, Executed::Failed);
    let failure = record
        .failures
        .get(&key)
        .unwrap_or_else(|| panic!("no failure for {key:?}: {:?}", record.failures));
    assert!(
        failure.message.contains('2') && !failure.message.is_empty(),
        "{failure:?}"
    );
    assert_eq!(failure.line, Some(1), "{failure:?}");
    assert_eq!(
        failure.cause, None,
        "an assertion is never infra: {failure:?}"
    );
    assert_eq!(failure.cut, None, "{failure:?}");
    assert_eq!(
        record.failures.len(),
        1,
        "a passing leaf has no words kept: {:?}",
        record.failures
    );
}

#[test]
fn an_infra_failure_is_recorded_with_its_typed_cause() {
    let tmp = Tmp::new("infra");
    package(&tmp, "prova.test(\"needs a disk\", function(t) prova.infra(\"the workspace cannot be read\") end)\n");
    let (out, record) = run(&tmp, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let failure = &record.failures[&leaf(&record, Executed::Failed)];
    assert_eq!(failure.cause.as_deref(), Some(CAUSE_INFRA), "{failure:?}");
    assert!(
        failure.message.contains("the workspace cannot be read"),
        "{failure:?}"
    );
}

#[test]
fn a_message_past_the_cap_keeps_its_tail_and_says_it_was_cut() {
    let tmp = Tmp::new("cut");
    package(&tmp, "prova.test(\"loud\", function(t) error(\"THE START \" .. string.rep(\"é\", 6000) .. \" THE END\") end)\n");
    let (out, record) = run(&tmp, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let failure = &record.failures[&leaf(&record, Executed::Failed)];
    assert!(
        failure.message.len() <= FAILURE_MESSAGE_AT_MOST,
        "{} bytes",
        failure.message.len()
    );
    assert!(
        failure.message.starts_with(FAILURE_CUT_MARK),
        "{:?}",
        &failure.message[..40]
    );
    assert!(
        failure.message.contains("THE END") && !failure.message.contains("THE START"),
        "the tail is kept"
    );
    assert!(failure.cut.is_some_and(|n| n > 0), "{:?}", failure.cut);
}

/// A flake keeps the words of the failure it passed after: the earlier run's, over the same tree.
#[test]
fn a_flaky_leaf_keeps_the_words_of_the_failure_it_passed_after() {
    let tmp = Tmp::new("flaky");
    package(&tmp, "prova.test(\"flips\", function(t)\n  if os.getenv(\"PROVA_ROW_FLIP\") ~= \"pass\" then error(\"the first try lost\") end\nend)\n");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .current_dir(&tmp.0)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    write(&tmp.0.join(".gitignore"), "record.json\n.x/\n");
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.email=p@example.com",
        "-c",
        "user.name=p",
        "commit",
        "-qm",
        "a package",
    ]);
    let (out, _) = run(&tmp, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let (out, record) = run(&tmp, &[("PROVA_ROW_FLIP", "pass")]);
    let key = leaf(&record, Executed::Flaky);
    let failure = record
        .failures
        .get(&key)
        .unwrap_or_else(|| panic!("{}: {:?}", said(&out), record.failures));
    assert!(
        failure.message.contains("the first try lost"),
        "{failure:?}"
    );
}
