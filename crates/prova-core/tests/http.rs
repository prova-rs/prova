use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;

use prova_core::{run_path_with, NullReporter, RunConfig};

/// A throwaway HTTP/1.1 server for testing the client: every request gets `200` and a fixed JSON
/// body, one request per connection (`Connection: close`). Test-only — the client under test is the
/// real, robust one. Returns the base URL; the listener thread lives until the process exits.
fn spawn_mock_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Drain the request headers (read the clone so we can still write to `stream`).
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }
            let body = br#"{"status":"ok","service":"orders"}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    base
}

/// End-to-end `http` module: GET status/body/json, POST with a JSON body, and `wait_for` — all
/// against a local server, driven from a `.lua` test through prova's async runtime.
#[test]
fn http_module_probes_a_service() {
    let base = spawn_mock_server();
    let test_lua = format!(
        r#"
local base = [[{base}]]

prova.test("GET returns 200 with a json body", function(t)
  local res = http.get(base .. "/health")
  t:expect(res.status):equals(200)
  t:expect(res.body):contains("ok")
  t:expect(res:json().status):equals("ok")
  t:expect(res:json().service):equals("orders")
end)

prova.test("POST sends a json body", function(t)
  local res = http.post(base .. "/orders", {{ json = {{ sku = "widget", qty = 2 }} }})
  t:expect(res.status):equals(200)
end)

prova.test("wait_for returns once healthy", function(t)
  local res = http.wait_for(base .. "/health", {{ status = 200, timeout = "5s", every = "50ms" }})
  t:expect(res.status):equals(200)
end)

prova.test("PATCH is supported", function(t)
  local res = http.patch(base .. "/orders/1", {{ json = {{ qty = 3 }} }})
  t:expect(res.status):equals(200)
end)

prova.test("client prefixes base_url and reuses default headers", function(t)
  local api = http.client{{ base_url = base, headers = {{ authorization = "Bearer t" }} }}
  local res = api:get("/health")            -- path joined onto base_url
  t:expect(res.status):equals(200)
  t:expect(res:json().service):equals("orders")
  local created = api:post("/orders", {{ json = {{ sku = "x" }} }})
  t:expect(created.status):equals(200)
  local ready = api:wait_for("/health", {{ status = 200, timeout = "5s", every = "50ms" }})
  t:expect(ready.status):equals(200)
end)
"#
    );

    let dir = std::env::temp_dir().join(format!("prova-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("http_test.lua");
    std::fs::write(&path, test_lua).unwrap();

    let mut reporter = NullReporter;
    let summary = run_path_with(&path, &mut reporter, &RunConfig::default()).expect("run");
    assert_eq!(summary.passed, 5, "passed");
    assert_eq!(summary.failed, 0, "failed");

    let _ = std::fs::remove_dir_all(&dir);
}
