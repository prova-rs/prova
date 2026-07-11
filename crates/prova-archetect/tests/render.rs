use std::path::PathBuf;

use prova_core::{run_path_with, NullReporter, RunConfig};

fn greeting_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/greeting")
}

/// Direct in-process render: headless with defaults writes the templated, parameter-named file.
#[test]
fn render_headless_writes_the_templated_file() {
    let src = greeting_fixture();
    let dest = std::env::temp_dir().join(format!("prova-archetect-direct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dest);

    let writes = prova_archetect::render_headless(
        src.to_str().unwrap(),
        &dest,
        prova_archetect::ContextMap::new(),
        vec![],
        true, // use defaults for every prompt
    )
    .expect("render");

    assert!(!writes.is_empty(), "the render reported file writes");
    let out = dest.join("demo.txt"); // {{ name }} defaulted to "demo"
    assert!(out.exists(), "rendered file exists at {}", out.display());
    let body = std::fs::read_to_string(&out).unwrap();
    assert!(body.contains("Hello, demo!"), "templated body: {body:?}");
    assert!(
        body.contains("Port: 8080"),
        "int default rendered: {body:?}"
    );

    let _ = std::fs::remove_dir_all(&dest);
}

/// End-to-end through the whole prova stack: a `.lua` test file drives `archetect.render{...}` (the
/// installed plugin module), then asserts on the tree handle with prova's fs matchers. Proves the
/// module wiring, answers-as-data, and the `out:file(...):read()`/`out.writes` handle surface.
#[test]
fn renders_through_the_archetect_module() {
    let src = greeting_fixture();
    // Generate the test file with the fixture path baked in (long-bracket string, no escaping).
    let test_lua = format!(
        r#"
local src = [[{src}]]

prova.test("renders with defaults", function(t)
  local out = archetect.render{{ source = src, destination = fs.tempdir(), defaults = true }}
  t:expect(out:file("demo.txt")):exists()
  t:expect(out:file("demo.txt"):read()):contains("Hello, demo!")
  t:expect(#out.writes):never():equals(0)
end)

prova.test("answers override defaults", function(t)
  local out = archetect.render{{
    source = src,
    destination = fs.tempdir(),
    answers = {{ name = "widget", port = 9090 }},
    defaults = true,
  }}
  t:expect(out:file("widget.txt")):exists()
  local body = out:file("widget.txt"):read()
  t:expect(body):contains("Hello, widget!")
  t:expect(body):contains("Port: 9090")
end)
"#,
        src = src.display()
    );

    let dir = std::env::temp_dir().join(format!("prova-archetect-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("render_test.lua");
    std::fs::write(&path, test_lua).unwrap();

    let config = RunConfig::new(1).with_module(prova_archetect::install);
    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &config).expect("run render_test.lua");

    assert_eq!(summary.passed, 2, "passed");
    assert_eq!(summary.failed, 0, "failed");

    let _ = std::fs::remove_dir_all(&dir);
}
