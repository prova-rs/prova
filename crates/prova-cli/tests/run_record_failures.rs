//! End-to-end (hermetic: no docker, no network): a red run's record keeps each red leaf's own words,
//! so a red can be read once the console is gone (E m/14546: Herald and Horai read the record, never
//! the scrollback). Each record is read back the consumer's way, through `ledger::read_record`.

use std::path::Path;
use std::process::Command;

use prova_core::ledger::{
    read_record, Executed, Record, CAUSE_INFRA, FAILURE_CUT_MARK, FAILURE_MESSAGE_AT_MOST,
    INFRA_NEVER_RAN,
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
    assert_eq!(
        failure.infra_kind, None,
        "an infra of no named kind: {failure:?}"
    );
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

/// nextest's account when it could not execute two cases (their binary gone under the run, as a pressure sweep
/// deleted herald's build directory, 2026-11-24), spawned directly (`NEXTEST_DOUBLE_SPAWN=0`): its own type.
const NEVER_RAN: &str = r#"<testsuites><testsuite name="k" tests="3" errors="2" failures="0">
<testcase name="ran" classname="k" time="0.010"></testcase>
<testcase name="gone_one" classname="k" time="0.000"><error message="error spawning child process" type="execution failure">error spawning child process</error></testcase>
<testcase name="gone_two" classname="k" time="0.000"><error message="error spawning child process" type="execution failure">error spawning child process</error></testcase>
</testsuite></testsuites>"#;
const ALL_GREEN: &str = r#"<testsuites><testsuite name="k" tests="3"><testcase name="ran" classname="k"/><testcase name="gone_one" classname="k"/><testcase name="gone_two" classname="k"/></testsuite></testsuites>"#;
const ONE_RED: &str = r#"<testsuites><testsuite name="k" tests="2"><testcase name="ran" classname="k"><failure message="assertion failed: it broke"/></testcase><testcase name="gone_one" classname="k" time="0.000"><error message="error spawning child process" type="execution failure"/></testcase></testsuite></testsuites>"#;

/// The account's verifier and a claim row that reads it, as herald's `spoken_for` does: the account chosen by env.
const DEPUTY_PROOF: &str = r#"local results = os.getenv("PROVA_ROW_ACCOUNT")
prova.test("the account holds", function(t) junit.verify(t, { results = results }) end)
prova.test("the claim is spoken for", function(t)
  for _, c in ipairs(junit.load(results).cases) do
    if c.name == "gone_one" then t:expect(c.outcome, c.name):equals("passed") end
  end
end)
"#;

/// A case its deputy could not execute never ran: the verifier fails as infra, by the deputy's own type, never red.
#[test]
fn a_case_its_deputy_could_not_execute_is_infra_never_red() {
    let tmp = Tmp::new("never-ran");
    package(&tmp, DEPUTY_PROOF);
    write(&tmp.0.join("never.xml"), NEVER_RAN);
    let (out, record) = run(&tmp, &[("PROVA_ROW_ACCOUNT", "never.xml")]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let verify = record
        .failures
        .iter()
        .find(|(k, _)| k.contains("the account holds"))
        .map(|(_, f)| f)
        .unwrap_or_else(|| panic!("{:?}", record.failures));
    assert_eq!(verify.cause.as_deref(), Some(CAUSE_INFRA), "{verify:?}");
    assert_eq!(
        verify.infra_kind.as_deref(),
        Some(INFRA_NEVER_RAN),
        "its kind, by value (L m/34449): {verify:?}"
    );
    assert!(
        verify.message.contains("2 deputed case(s) never ran")
            && verify.message.contains("k#gone_one"),
        "{verify:?}"
    );
    let outcomes: Vec<(&str, &str)> = record
        .deputed
        .iter()
        .map(|d| (d.name.as_str(), d.outcome.as_str()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("ran", "passed"),
            ("gone_one", "infra"),
            ("gone_two", "infra")
        ],
        "{outcomes:?}"
    );
}

/// What ran and failed is evidence: a red beside a case that never ran stays red, with the infra said.
#[test]
fn a_red_beside_a_case_that_never_ran_stays_red() {
    let tmp = Tmp::new("red-beside");
    package(&tmp, DEPUTY_PROOF);
    write(&tmp.0.join("red.xml"), ONE_RED);
    let (out, record) = run(&tmp, &[("PROVA_ROW_ACCOUNT", "red.xml")]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let verify = record
        .failures
        .iter()
        .find(|(k, _)| k.contains("the account holds"))
        .map(|(_, f)| f)
        .unwrap_or_else(|| panic!("{:?}", record.failures));
    assert_eq!(verify.cause, None, "a red is red: {verify:?}");
    assert!(verify.message.contains("it broke"), "{verify:?}");
    assert!(
        said(&out).contains("1 deputed case(s) never ran"),
        "{}",
        said(&out)
    );
}

/// The run whose deputy could not execute judged on evidence that never ran, so it lends nothing: the next run on the
/// same tree passing is green, never a hundred flakes (herald 091bc3e, 1db5a83). Its claim row's red lends nothing
/// either, since the journal is that same run's.
#[test]
fn a_run_whose_deputy_could_not_execute_lends_no_flake() {
    let tmp = Tmp::new("lends-none");
    package(&tmp, DEPUTY_PROOF);
    write(&tmp.0.join("never.xml"), NEVER_RAN);
    write(&tmp.0.join("green.xml"), ALL_GREEN);
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
    write(&tmp.0.join(".gitignore"), "record.json\n.x/\n.prova/\n");
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
    let (out, _) = run(&tmp, &[("PROVA_ROW_ACCOUNT", "never.xml")]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let (out, record) = run(&tmp, &[("PROVA_ROW_ACCOUNT", "green.xml")]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert!(
        !record.executed.values().any(|o| *o == Executed::Flaky),
        "no leaf flaky: {:?}",
        record.executed
    );
    assert!(
        record.deputed.iter().all(|d| d.outcome == "passed"),
        "{:?}",
        record.deputed
    );
}

/// L m/34449: the record names the build dir whose lease the run held, so a gate tells two runs' build dirs apart by
/// value; a run with no cargo project holds none and names none.
#[test]
fn the_record_names_the_build_dir_whose_lease_the_run_held() {
    let tmp = Tmp::new("build-dir");
    package(&tmp, "prova.test(\"one\", function(t) end)\n");
    let (out, record) = run(&tmp, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(
        record.build_dir, None,
        "no cargo project, no lease, none named"
    );
    write(
        &tmp.0.join("Cargo.toml"),
        "[package]\nname = \"lease-row\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write(&tmp.0.join("src/lib.rs"), "");
    let target = tmp.0.join("built");
    // Hermetic (rule 10): the row's build dir is its own, never the host's configured build-dir on its build volume.
    let at = target.to_str().unwrap();
    let (out, record) = run(
        &tmp,
        &[("CARGO_TARGET_DIR", at), ("CARGO_BUILD_BUILD_DIR", at)],
    );
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    let named = record
        .build_dir
        .unwrap_or_else(|| panic!("a held lease names its dir: {}", said(&out)));
    assert!(
        Path::new(&named).join(".space-lease").is_file() && named.ends_with("built"),
        "the dir whose lease it held: {named}"
    );
}
