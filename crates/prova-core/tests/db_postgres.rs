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

/// The `db` module against a REAL Postgres in an ephemeral container — the same API verified with
/// SQLite (tests/db.rs), differing only in the connect URL. Runs for real where docker is present,
/// skips (via `requires`) where it is absent. Either way, nothing fails.
#[test]
fn db_module_queries_real_postgres_or_skips() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/db_postgres_test.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run db_postgres_test.lua");

    assert_eq!(
        summary.failed, 0,
        "never fails, whether or not docker is present"
    );
    if docker_available() {
        assert_eq!(
            summary.passed, 1,
            "the postgres round-trip passes with docker present"
        );
    } else {
        assert_eq!(
            summary.skipped, 1,
            "skips (requires docker) when it is absent"
        );
    }
}
