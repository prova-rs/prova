# drivers — speaking the SUT's protocol from the proof

A driver is how a proof ORIGINATES traffic at the system under test. The rule: **drive the
contract under proof with the protocol of that contract** — a gRPC API through a gRPC client,
not curl-shaped workarounds; a CLI through its argv; a filesystem effect through the
filesystem. Green must mean "a real caller would succeed."

| Contract under proof | Driver | Core moves |
|---|---|---|
| HTTP/REST | `http` | `http.get/post(url, { headers, json, timeout })` → `.status`, `.body`, `:json()` · `http.client{ base_url }` · `http.wait_for(url, { status, timeout })` |
| gRPC | `grpc` | `grpc.client(addr)` → `:call(method, req)`, `:call_status` (needs server reflection) · `grpc.wait_for` |
| GraphQL | `graphql` | `graphql.client{ url }` → `:query`, `:execute` |
| CLI / processes | `shell` | `shell.run(cmd_or_argv, { cwd, env, timeout, check })` → `{ code, stdout, stderr }` · `shell.spawn` for long-running |
| SQL state (cross-check) | `sqlite.client(url)`, or the resource plugin's `client` (postgres/mysql…) | assert effects WHERE THEY LAND |
| Files / rendered trees | `fs` | `read write exists glob tempdir remove_all` · snapshot the tree |
| Containers (exec inside) | `docker` | `container:run(argv)`, `:exec`, `:logs` |

## Choosing, quickly

- Proving a service's API contract → the protocol driver for that API. Cross-check the side
  effect with a second driver (query the DB, read the file) — one action, asserted at both
  boundaries.
- Proving a CLI → `shell.run` with the ARGV form (`{ "bin", "--flag", value }` — no quoting
  hazards; a string command is fine when fixed).
- Proving a rendered/built artifact → `fs` + `matches_snapshot` (layout or content level).
- Readiness is a driver call that HOLDS (`http.wait_for`, a query succeeding) — never a sleep.

## Boundaries

- Drivers originate; **doubles** stand in for what the SUT calls out to (`prova learn
  doubles`); **proxies** (interpose) are not yet a shipped surface (`prova learn proxies`).
- `http`/`grpc` responses are userdata, not tables — use `:json()` and fields, don't iterate.
  When a shape surprises you: `prova.help("HttpResponse")` or probe with `eval`.
- A protocol prova doesn't speak natively: drive the official CLI via `shell.run` argv, or
  wrap the SDK in a plugin (`prova learn plugin-authoring`).
