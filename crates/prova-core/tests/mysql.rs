use std::path::PathBuf;
use std::process::{Command, Stdio};

use prova_core::{run_path, NullReporter};

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The `mysql.container` recipe against a REAL MySQL in an ephemeral container — the same query API
/// as the Postgres/SQLite tests, differing only in the recipe call and `?` placeholders. Runs for
/// real where docker is present, skips (via `requires`) where it is absent. Either way, nothing fails.
#[test]
fn mysql_container_recipe_queries_real_mysql_or_skips() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/mysql_test.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run mysql_test.lua");

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(summary.passed, 1, "the mysql round-trip passes with docker");
    } else {
        assert_eq!(summary.skipped, 1, "skips (requires docker) when absent");
    }
}
