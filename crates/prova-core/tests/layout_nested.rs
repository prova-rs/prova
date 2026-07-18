use std::path::PathBuf;

use prova_core::discover_suites;

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// Directory-scoped discovery: a `suite.lua` owns the `*_test.lua` in ITS directory only; a nested
/// `suite.lua` is its own suite, not swallowed.
///
/// RED today — `collect_suites` grabs the whole subtree the moment it sees a `suite.lua`, so `outer/`
/// absorbs `inner/b_test.lua` and `inner/suite.lua` is ignored: one suite spanning both directories.
/// The target is two suites, neither spanning the nested boundary.
#[test]
fn nested_suite_lua_are_two_suites_not_one() {
    let suites = discover_suites(&testdata("layout_nested")).expect("discover");
    assert_eq!(suites.len(), 2, "outer and inner are two suites, not one");
    for s in &suites {
        let has_a = s.files.iter().any(|f| f.ends_with("a_test.lua"));
        let has_b = s.files.iter().any(|f| f.ends_with("b_test.lua"));
        assert!(
            !(has_a && has_b),
            "no suite may span the nested suite.lua boundary"
        );
    }
}
