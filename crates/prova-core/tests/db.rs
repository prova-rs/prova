use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// The `db` module over SQLite (no server needed): connect, execute (rows affected), query
/// (column-mapped rows with typed values), query_value (scalar + NULL→nil), positional params. The
/// same API drives Postgres/MySQL — only the connect URL differs.
#[test]
fn db_module_queries_sqlite() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/db.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run db.lua");
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
