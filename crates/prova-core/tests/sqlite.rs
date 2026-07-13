use std::path::PathBuf;

use prova_core::{run_path, NullReporter};

/// The `sqlite` module (no server needed): `sqlite.client`, execute (rows affected), query
/// (column-mapped rows with typed values), query_value (scalar + NULL→nil), positional params. The
/// same API drives Postgres/MySQL — only the namespace and URL differ.
#[test]
fn sqlite_module_queries_sqlite() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/sqlite.lua");
    let mut reporter = NullReporter;
    let summary = run_path(&path, &mut reporter).expect("run sqlite.lua");
    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 0, "failed");
}
