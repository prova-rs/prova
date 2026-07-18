//! `inspect_plugin` — the engine behind `prova plugin lint`: load a plugin file, evaluate it to its
//! returned namespace, and check it against the namespacing grammar.

use std::path::PathBuf;

use prova_core::{inspect_plugin, PluginShape, RunConfig};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/plugin_lint")
        .join(name)
}

#[test]
fn resource_plugin_reports_facets_and_no_issues() {
    let report = inspect_plugin(&fixture("good.lua"), &RunConfig::new(1)).expect("inspect");
    assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
    assert_eq!(report.shape, Some(PluginShape::Resource));
    assert!(
        report.facets.contains(&"container".to_string()),
        "{:?}",
        report.facets
    );
    assert!(
        report.facets.contains(&"client".to_string()),
        "{:?}",
        report.facets
    );
}

#[test]
fn library_plugin_with_no_facets_is_valid() {
    // A plain table of helpers (no resource facets) is a valid library plugin, not an error.
    let report = inspect_plugin(&fixture("no_facets.lua"), &RunConfig::new(1)).expect("inspect");
    assert!(
        report.issues.is_empty(),
        "a library is valid; issues: {:?}",
        report.issues
    );
    assert_eq!(report.shape, Some(PluginShape::Library));
    assert!(report.facets.is_empty());
}

#[test]
fn returning_a_non_table_is_an_issue() {
    let report = inspect_plugin(&fixture("not_a_table.lua"), &RunConfig::new(1)).expect("inspect");
    assert!(
        report.issues.iter().any(|i| i.contains("namespace table")),
        "issues: {:?}",
        report.issues
    );
}

#[test]
fn a_non_function_facet_is_an_issue() {
    let report = inspect_plugin(&fixture("bad_facet.lua"), &RunConfig::new(1)).expect("inspect");
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.contains("should be a function")),
        "issues: {:?}",
        report.issues
    );
}
