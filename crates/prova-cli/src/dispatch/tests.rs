//! The dispatcher's flag grammar, proven at the parse — every axis and every refusal at the door.

use super::*;

fn parse(args: &[&str]) -> Result<Cli, ExitCode> {
    parse_cli(args.iter().map(|s| s.to_string()).collect())
}

/// The run-shaping grammar in one pass: every selection axis with its `!` exclusion form,
/// comma-splitting where lists are natural, and the value flags that name what runs.
#[test]
fn parse_cli_reads_the_selection_axes_and_run_shape() {
    let cli = parse(&[
        "-k", "orders", "-k", "!slow-orders",
        "--tags", "unit, http ,!flaky",
        "--node", "engine › selects",
        "-j", "4", "-p", "quality", "-s", "ci,docker", "--switch", "soak",
        "-P", "pg=./packages/pg", "--record", "out.json", "--topology", "dev",
        "--heed=ops,security", "--heed=ops",
        "proofs/engine",
    ])
    .ok()
    .unwrap();
    assert_eq!(cli.selection.keywords, vec!["orders"]);
    assert_eq!(cli.selection.keyword_excludes, vec!["slow-orders"]);
    assert_eq!(cli.selection.tags, vec!["unit", "http"]);
    assert_eq!(cli.selection.tag_excludes, vec!["flaky"]);
    assert_eq!(cli.selection.nodes, vec!["engine › selects"]);
    assert_eq!(cli.jobs, Some(4));
    assert_eq!(cli.profile.as_deref(), Some("quality"));
    assert_eq!(cli.switches, vec!["ci", "docker", "soak"]);
    assert_eq!(cli.packages, vec!["pg=./packages/pg"]);
    assert_eq!(cli.record_to.as_deref(), Some(std::path::Path::new("out.json")));
    assert_eq!(cli.require_topology.as_deref(), Some("dev"));
    assert_eq!(
        cli.heed,
        crate::manifest::Heed::Matching(vec!["ops".into(), "security".into()]),
        "heed selectors accumulate and dedupe across flags"
    );
    assert_eq!(cli.explicit_paths, vec!["proofs/engine"], "bare paths are the selection");
}

/// Every taught refusal exits 2 at the parse, before anything loads: a non-positive job
/// count, and unknown --format/--color/--progress spellings.
#[test]
fn parse_cli_refuses_bad_values_at_the_door() {
    for bad in [
        &["--jobs", "0"][..],
        &["--jobs", "many"][..],
        &["--format", "yaml"][..],
        &["--color", "sometimes"][..],
        &["--progress", "maybe"][..],
    ] {
        assert!(parse(bad).is_err(), "{bad:?} must be refused");
    }
    let cli = parse(&["--format", "json", "--color", "never", "--progress", "always"])
        .ok()
        .unwrap();
    assert!(matches!(cli.format, Some(Format::Json)));
    assert!(matches!(cli.color, Some(report::ColorMode::Never)));
}
