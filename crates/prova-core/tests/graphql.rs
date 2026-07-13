use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use prova_core::{run_path_with, NullReporter, RunConfig};

/// A throwaway GraphQL-over-HTTP server: reads the POST body and returns `{errors:[…]}` when the
/// query contains "fail", otherwise `{data:{hello:"world"}}`. Test-only.
fn spawn_gql_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let mut content_length = 0usize;
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body);
            let resp_body: &[u8] = if String::from_utf8_lossy(&body).contains("fail") {
                br#"{"errors":[{"message":"boom"}]}"#
            } else {
                br#"{"data":{"hello":"world"}}"#
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp_body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(resp_body);
            let _ = stream.flush();
        }
    });
    base
}

/// The `graphql` module: `:query` returns `data` and raises on GraphQL errors; `:execute` returns the
/// full `{ data, errors, status }` envelope without raising.
#[test]
fn graphql_client_query_and_execute() {
    let url = spawn_gql_server();
    let test_lua = format!(
        r#"
local api = graphql.client{{ url = [[{url}]] }}

prova.test("query returns data", function(t)
  local data = api:query("{{ hello }}")
  t:expect(data.hello):equals("world")
end)

prova.test("query raises on graphql errors", function(t)
  local ok = pcall(function() api:query("{{ fail }}") end)
  t:expect(ok):is_false()
end)

prova.test("execute returns the full envelope without raising", function(t)
  local res = api:execute("{{ fail }}")
  t:expect(res.status):equals(200)
  t:expect(res.errors):never():is_nil()
  t:expect(res.data):is_nil()
end)
"#
    );

    let dir = std::env::temp_dir().join(format!("prova-gql-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("graphql_test.lua");
    std::fs::write(&path, test_lua).unwrap();

    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &RunConfig::default()).expect("run");
    assert_eq!(summary.passed, 3, "passed");
    assert_eq!(summary.failed, 0, "failed");

    let _ = std::fs::remove_dir_all(&dir);
}
