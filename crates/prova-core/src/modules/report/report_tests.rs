//! `report.publish` — the record it files, and the custody it takes.

use super::*;

fn scratch(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "prova-report-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn env(custody: Option<std::path::PathBuf>) -> (Lua, ReportRegistry) {
    let lua = Lua::new();
    let registry: ReportRegistry = Default::default();
    let report = make(&lua, Some(registry.clone()), custody).unwrap();
    lua.globals().set("report", report).unwrap();
    (lua, registry)
}

fn published(registry: &ReportRegistry) -> Vec<Report> {
    registry.lock().unwrap().clone()
}

/// The record a conduct files: an address, the one line prova renders, the measurement it explains,
/// and every form sorted by kind so a chooser sees a stable list.
#[test]
fn publish_files_the_report_with_its_forms() {
    let dir = scratch("basic");
    std::fs::write(dir.join("cov.json"), "{}").unwrap();
    std::fs::write(dir.join("index.html"), "<html>").unwrap();
    let (lua, registry) = env(None);
    lua.globals().set("DIR", dir.to_str().unwrap()).unwrap();

    lua.load(
        r#"
        report.publish{
          name = "coverage",
          summary = "unit 73.47% · merged 86.37%",
          explains = "rust.coverage.unit",
          forms = { json = DIR .. "/cov.json", html = DIR .. "/index.html" },
        }
        "#,
    )
    .exec()
    .unwrap();

    let rows = published(&registry);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "coverage");
    assert_eq!(rows[0].summary, "unit 73.47% · merged 86.37%");
    assert_eq!(rows[0].explains, vec!["rust.coverage.unit"]);
    let kinds: Vec<_> = rows[0].forms.iter().map(|f| f.kind.as_str()).collect();
    assert_eq!(kinds, vec!["html", "json"], "forms are sorted by kind");
}

/// Both spellings mean the same thing — the map reads better when each kind appears once, the list
/// is there for when it does not.
#[test]
fn the_list_and_map_spellings_agree() {
    let dir = scratch("spellings");
    std::fs::write(dir.join("a.json"), "{}").unwrap();
    let (lua, registry) = env(None);
    lua.globals().set("P", dir.join("a.json").to_str().unwrap()).unwrap();

    lua.load(r#"report.publish{ name="m", summary="s", forms = { json = P } }"#).exec().unwrap();
    lua.load(r#"report.publish{ name="l", summary="s", forms = { { kind="json", path=P } } }"#)
        .exec()
        .unwrap();

    let rows = published(&registry);
    let m = rows.iter().find(|r| r.name == "m").unwrap();
    let l = rows.iter().find(|r| r.name == "l").unwrap();
    assert_eq!(m.forms.len(), 1);
    assert_eq!(m.forms[0].kind, l.forms[0].kind);
    assert_eq!(m.forms[0].path, l.forms[0].path);
}

/// Custody is the whole point: `target/` is swept and a tempdir is reaped, so a recorded path that
/// still points into the conduct is a link that rots — and a rotted report is worse than none,
/// because it reads as available.
#[test]
fn custody_copies_the_artifact_out_of_the_conducts_reach() {
    let produced = scratch("produced");
    let custody = scratch("custody");
    std::fs::write(produced.join("cov.json"), "{\"lines\":1}").unwrap();
    let (lua, registry) = env(Some(custody.clone()));
    lua.globals().set("P", produced.join("cov.json").to_str().unwrap()).unwrap();

    lua.load(r#"report.publish{ name="coverage", summary="s", forms = { json = P } }"#)
        .exec()
        .unwrap();

    let rows = published(&registry);
    let filed = &rows[0].forms[0].path;
    assert!(filed.starts_with(&custody), "filed under custody: {}", filed.display());
    assert_eq!(std::fs::read_to_string(filed).unwrap(), "{\"lines\":1}");

    // The conduct's copy going away does not take the report with it — the case custody exists for.
    std::fs::remove_dir_all(&produced).unwrap();
    assert!(filed.exists(), "the filed copy outlives what produced it");
}

/// llvm-cov's HTML report is a TREE of pages, and half of one is not a report.
#[test]
fn custody_copies_a_directory_whole() {
    let produced = scratch("tree");
    let custody = scratch("tree-custody");
    std::fs::create_dir_all(produced.join("html/sub")).unwrap();
    std::fs::write(produced.join("html/index.html"), "<html>").unwrap();
    std::fs::write(produced.join("html/sub/page.html"), "<p>").unwrap();
    let (lua, registry) = env(Some(custody.clone()));
    lua.globals().set("P", produced.join("html").to_str().unwrap()).unwrap();

    lua.load(r#"report.publish{ name="coverage", summary="s", forms = { html = P } }"#)
        .exec()
        .unwrap();

    let filed = published(&registry)[0].forms[0].path.clone();
    // The address is the tree's ENTRY POINT, so `open $(…)` lands on the report rather than on a
    // directory listing — while the tree itself came along, links intact.
    assert_eq!(filed.file_name().unwrap(), "index.html", "addressed at its door: {}", filed.display());
    let root = filed.parent().unwrap();
    assert!(root.join("sub/page.html").exists(), "…and the whole tree came with it");
}

/// A directory with no `index.html` has no door to name, so the directory itself is the address —
/// the convention applies where it holds and is not invented where it does not.
#[test]
fn a_tree_without_an_entry_point_is_addressed_as_a_tree() {
    let produced = scratch("no-index");
    let custody = scratch("no-index-custody");
    std::fs::create_dir_all(produced.join("lcov")).unwrap();
    std::fs::write(produced.join("lcov/a.info"), "TN:").unwrap();
    let (lua, registry) = env(Some(custody));
    lua.globals().set("P", produced.join("lcov").to_str().unwrap()).unwrap();

    lua.load(r#"report.publish{ name="cov", summary="s", forms = { lcov = P } }"#).exec().unwrap();

    let filed = published(&registry)[0].forms[0].path.clone();
    assert!(filed.is_dir(), "no index.html, so the tree is the address: {}", filed.display());
    assert!(filed.join("a.info").exists());
}

/// A conduct that runs twice in one run must leave ONE current report, not two rows disagreeing
/// about which is the answer.
#[test]
fn republishing_a_name_replaces_it() {
    let dir = scratch("replace");
    std::fs::write(dir.join("a.json"), "{}").unwrap();
    let (lua, registry) = env(None);
    lua.globals().set("P", dir.join("a.json").to_str().unwrap()).unwrap();

    lua.load(r#"report.publish{ name="coverage", summary="first", forms = { json = P } }"#)
        .exec()
        .unwrap();
    lua.load(r#"report.publish{ name="coverage", summary="second", forms = { json = P } }"#)
        .exec()
        .unwrap();

    let rows = published(&registry);
    assert_eq!(rows.len(), 1, "one row per name");
    assert_eq!(rows[0].summary, "second", "the later publish is the current answer");
}

/// The refusals, each naming what is missing. A report with no summary is a path with extra steps;
/// one with no forms is a log line; an unknown key is the closed-table contract every module holds.
#[test]
fn a_report_that_could_not_be_read_is_refused() {
    let dir = scratch("refuse");
    std::fs::write(dir.join("a.json"), "{}").unwrap();
    let (lua, _registry) = env(None);
    lua.globals().set("P", dir.join("a.json").to_str().unwrap()).unwrap();

    let err = |src: &str| lua.load(src).exec().unwrap_err().to_string();

    let no_summary = err(r#"report.publish{ name="c", summary="  ", forms = { json = P } }"#);
    assert!(no_summary.contains("`summary` is required"), "got: {no_summary}");

    let no_forms = err(r#"report.publish{ name="c", summary="s", forms = {} }"#);
    assert!(no_forms.contains("no `forms`"), "got: {no_forms}");

    let no_name = err(r#"report.publish{ name="", summary="s", forms = { json = P } }"#);
    assert!(no_name.contains("cannot be empty"), "got: {no_name}");

    let typo = err(r#"report.publish{ name="c", summary="s", form = { json = P } }"#);
    assert!(typo.contains("form"), "an unknown key is named, not dropped: {typo}");
}

/// With no registry attached — an `eval`, a bare embedder — publishing is a no-op rather than an
/// error, matching `measure.record`, so it never leaks outside a run.
#[test]
fn publishing_outside_a_run_is_a_no_op() {
    let dir = scratch("noop");
    std::fs::write(dir.join("a.json"), "{}").unwrap();
    let lua = Lua::new();
    let report = make(&lua, None, None).unwrap();
    lua.globals().set("report", report).unwrap();
    lua.globals().set("P", dir.join("a.json").to_str().unwrap()).unwrap();

    lua.load(r#"report.publish{ name="c", summary="s", forms = { json = P } }"#)
        .exec()
        .expect("a no-op, not a failure");
}

/// An artifact that cannot be filed leaves the report pointing at where it actually is. A proof
/// does not deserve to go red because its evidence could not be copied.
#[test]
fn an_unfilable_artifact_is_reported_where_it_lies() {
    let custody = scratch("missing-custody");
    let (lua, registry) = env(Some(custody.clone()));
    lua.globals().set("P", "/nonexistent/nowhere/cov.json").unwrap();

    lua.load(r#"report.publish{ name="coverage", summary="s", forms = { json = P } }"#)
        .exec()
        .unwrap();

    let filed = published(&registry)[0].forms[0].path.clone();
    assert_eq!(filed, std::path::PathBuf::from("/nonexistent/nowhere/cov.json"));
}
