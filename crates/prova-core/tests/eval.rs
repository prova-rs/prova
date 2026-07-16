//! `eval_snippet` — the engine entry behind `prova eval`: a one-shot snippet in the full
//! environment (expression or statements), a real transient `ctx`, teardown after the snippet,
//! and defensive JSON conversion of the result. Hermetic (no docker, no network).

use serde_json::json;

use prova_core::{eval_snippet, RunConfig};

fn eval(code: &str) -> mlua::Result<serde_json::Value> {
    eval_snippet(code, &RunConfig::new(1))
}

#[test]
fn bare_expressions_and_explicit_returns_both_evaluate() {
    // Bare expression: the `return (…)` wrapping compiles.
    assert_eq!(eval("1 + 1").unwrap(), json!(2));
    // Explicit return: the wrapping fails to compile, the raw source runs.
    assert_eq!(eval("return 1 + 1").unwrap(), json!(2));
    // Multi-statement snippet with its own return.
    assert_eq!(eval("local x = 2\nreturn x * 3").unwrap(), json!(6));
    // A trailing comment must not swallow the expression wrapper.
    assert_eq!(eval("40 + 2 -- the answer").unwrap(), json!(42));
}

#[test]
fn results_convert_to_json_defensively() {
    // nil → null (statements with no return also yield nil).
    assert_eq!(eval("return nil").unwrap(), json!(null));
    assert_eq!(eval("local _ = 1").unwrap(), json!(null));
    // Scalars.
    assert_eq!(eval("return true").unwrap(), json!(true));
    assert_eq!(eval("return 1.5").unwrap(), json!(1.5));
    assert_eq!(eval("return 'hi'").unwrap(), json!("hi"));
    // A pure sequence is a JSON array; a keyed table an object; nesting works.
    assert_eq!(eval("return { 1, 2, 3 }").unwrap(), json!([1, 2, 3]));
    assert_eq!(
        eval("return { name = 'prova', tags = { 'a', 'b' } }").unwrap(),
        json!({ "name": "prova", "tags": ["a", "b"] })
    );
    // A function has no JSON form → its tostring(), not an error.
    let f = eval("return function() end").unwrap();
    assert!(
        f.as_str().is_some_and(|s| s.contains("function")),
        "unserializable values degrade to tostring(): {f}"
    );
    // A cyclic table must not hang or panic.
    let cyclic = eval("local t = {}; t.self = t; return t").unwrap();
    assert!(cyclic.is_object(), "cyclic table still reports: {cyclic}");
}

#[test]
fn the_full_environment_is_available() {
    // Built-in modules (fs) and the prova global are installed.
    let dir = std::env::temp_dir();
    let code = format!("return fs.exists({:?})", dir.to_string_lossy());
    assert_eq!(eval(&code).unwrap(), json!(true));
    assert_eq!(eval("return type(prova.retry)").unwrap(), json!("function"));
    // Async built-ins run (the snippet is driven through the async call path).
    assert_eq!(eval("prova.sleep(5); return 'awaited'").unwrap(), json!("awaited"));
}

#[test]
fn ctx_is_a_real_scope_and_tears_down_after_the_snippet() {
    // `ctx:tempdir()` allocates a real directory tied to the transient scope; `ctx:defer`
    // registers teardown. Both must have run by the time eval_snippet returns.
    let marker = std::env::temp_dir().join(format!("prova-eval-teardown-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let code = format!(
        "local dir = ctx:tempdir()\n\
         ctx:defer(function()\n\
         \x20 local f = io.open({marker:?}, 'w'); f:write(dir); f:close()\n\
         end)\n\
         return dir",
        marker = marker.to_string_lossy()
    );
    let value = eval(&code).unwrap();
    let dir = value.as_str().expect("tempdir path returned").to_string();
    assert!(marker.is_file(), "ctx:defer teardown ran after the snippet");
    assert!(
        !std::path::Path::new(&dir).exists(),
        "the transient scope's tempdir was removed on teardown"
    );
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn teardown_runs_even_when_the_snippet_raises() {
    let marker = std::env::temp_dir().join(format!("prova-eval-err-td-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let code = format!(
        "ctx:defer(function()\n\
         \x20 local f = io.open({marker:?}, 'w'); f:write('down'); f:close()\n\
         end)\n\
         error('boom')",
        marker = marker.to_string_lossy()
    );
    let err = eval(&code).expect_err("the snippet raises");
    assert!(err.to_string().contains("boom"), "{err}");
    assert!(marker.is_file(), "teardown ran despite the error");
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn a_raising_snippet_is_an_error() {
    assert!(eval("error('kaput')").is_err());
    // …and so is a snippet that doesn't compile either way.
    assert!(eval("this is not lua").is_err());
}
