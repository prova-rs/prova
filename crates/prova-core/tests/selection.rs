use std::path::PathBuf;

use prova_core::{run_path_with, NullReporter, RunConfig, Selection};

fn run_with(sel: Selection) -> prova_core::Summary {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/selection.lua");
    let mut config = RunConfig::new(1);
    config.selection = sel;
    let mut reporter = NullReporter;
    run_path_with(&path, &mut reporter, &config).expect("run selection.lua")
}

// The fixture has 6 leaves: alpha, bravo, charlie(dep→bravo), tagged›delta(slow), echo(fast),
// foxtrot flow (2 steps, one leaf).

#[test]
fn empty_selection_runs_everything() {
    let s = run_with(Selection::default());
    assert_eq!((s.passed, s.deselected), (7, 0)); // 5 tests + 2 flow steps report
}

#[test]
fn keyword_selects_by_path_substring() {
    let s = run_with(Selection {
        keywords: vec!["alpha".into()],
        ..Default::default()
    });
    assert_eq!(s.passed, 1);
    assert_eq!(s.deselected, 5); // five other leaves (the flow counts once)
}

#[test]
fn dependencies_are_pulled_in() {
    let s = run_with(Selection {
        keywords: vec!["charlie".into()],
        ..Default::default()
    });
    // charlie selected → bravo pulled in as its gate.
    assert_eq!(s.passed, 2);
    assert_eq!(s.deselected, 4);
}

#[test]
fn tags_match_inherited_group_tags() {
    let s = run_with(Selection {
        tags: vec!["slow".into()],
        ..Default::default()
    });
    assert_eq!(s.passed, 1); // delta via the group's tag
    assert_eq!(s.deselected, 5);
}

#[test]
fn tag_excludes_narrow() {
    let s = run_with(Selection {
        tag_excludes: vec!["slow".into()],
        ..Default::default()
    });
    assert_eq!(s.passed, 6); // everything but delta (flow = 2 passes)
    assert_eq!(s.deselected, 1);
}

#[test]
fn keyword_exclude_narrows() {
    let s = run_with(Selection {
        keyword_excludes: vec!["flow".into()],
        ..Default::default()
    });
    assert_eq!(s.passed, 5);
    assert_eq!(s.deselected, 1); // the flow leaf
}

#[test]
fn flows_are_atomic_under_selection() {
    let s = run_with(Selection {
        keywords: vec!["second step".into()],
        ..Default::default()
    });
    // Matching one step selects the whole flow: both steps run.
    assert_eq!(s.passed, 2);
    assert_eq!(s.deselected, 5);
}

#[test]
fn exact_node_selection() {
    let s = run_with(Selection {
        nodes: vec!["echo fast".into()],
        ..Default::default()
    });
    assert_eq!(s.passed, 1);
    assert_eq!(s.deselected, 5);
}
