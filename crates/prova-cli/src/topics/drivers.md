# drivers — speaking the SUT's protocol from the proof

A driver is how a proof ORIGINATES traffic at the system under test. The rule: **drive the
contract under proof with the protocol of that contract** — a gRPC API through a gRPC client,
not curl-shaped workarounds; a CLI through its argv; a filesystem effect through the
filesystem. Green must mean "a real caller would succeed."

| Contract under proof | Driver | Core moves |
|---|---|---|
| HTTP/REST | `http` | `http.get/post(url, { headers, json\|form\|body, content_type, timeout, redirects })` → `.status`, `.body` (bytes-exact), `:json()`, `:save(path)` · `http.client{ base_url }` · `http.wait_for(url, { status, headers, timeout })` |
| gRPC | `grpc` | `grpc.client(addr)` → `:call(method, req)`, `:call_status` (needs server reflection) · `grpc.wait_for` |
| GraphQL | `graphql` | `graphql.client{ url }` → `:query`, `:execute` |
| CLI / processes | `shell` | `shell.run(cmd_or_argv, { cwd, env, timeout, check })` → `{ code, stdout, stderr }` · `shell.spawn` for long-running |
| stdio conversations (MCP, LSP, REPLs) | `stdio` | `stdio.spawn(ctx, { cmd, framing, codec })` → `:send`, `:recv{ where }`, `:expect`, `:stderr`, `:eof`, `:wait` |
| Byte streams (tcp/unix) | `socket` | `socket.connect(ctx, { addr, framing, codec })` → `:send`, `:recv{ where }` |
| Interactive TUIs (pty) | `terminal` | `terminal.spawn(ctx, { cmd, cols, rows })` → `:send`, `:expect`, `:screen()` |
| SQL state (cross-check) | `sqlite.client(url)`, or the resource package's `client` (postgres/mysql…) | assert effects WHERE THEY LAND |
| Files / rendered trees | `fs` | `read write exists glob tempdir remove_all` · snapshot the tree |
| Containers (exec inside) | `docker` | `container:run(argv)`, `:exec`, `:logs` |

## Choosing, quickly

- Proving a service's API contract → the protocol driver for that API. Cross-check the side
  effect with a second driver (query the DB, read the file) — one action, asserted at both
  boundaries.
- Proving a CLI → `shell.run` with the ARGV form (`{ "bin", "--flag", value }` — no quoting
  hazards; a string command is fine when fixed).
- Proving something whose protocol is a CONVERSATION — where the next thing you send depends on
  what came back — → `stdio.spawn`. `shell.run{ stdin = … }` writes one string before the program
  starts, so it cannot express an exchange; batching the whole session instead is a race, not a
  workaround. Reach for `terminal.spawn` only when the SUT needs a real pty (a TUI with a screen);
  a pty mangles a byte protocol through line discipline and column wrapping.
- Proving a rendered/built artifact → `fs` + `matches_snapshot` (layout or content level).
- Readiness is a driver call that HOLDS (`http.wait_for`, a query succeeding) — never a sleep.
- Every stream driver shares one turn model: `framing` cuts bytes into turns (`"line"`,
  `"content_length"`, `{ delimiter }`, `{ length_prefixed }`), `codec = "json"` decodes them, and
  `recv{ where = { id = 3 } }` then reads on until the turn that MATCHES — the same structural
  subset match as `:matches` and `received()`. That is how you pick a reply out of a stream
  carrying interleaved notifications, without a hand-rolled read-until loop.

## Boundaries

- Drivers originate; **doubles** stand in for what the SUT calls out to (`prova learn doubles`);
  **proxies** interpose (`prova learn proxies`). When the SUT SPAWNS its dependency rather than
  dialing it, the double is spawnable: `stdio.mock`/`stdio.proxy` shadow a command name on PATH,
  and `prova relay` is the two-line adapter behind them.
- `http`/`grpc` responses are userdata, not tables — use `:json()` and fields, don't iterate.
  When a shape surprises you: `prova.help("HttpResponse")` or probe with `eval`.
- A binary payload needs no special verb: `.body` is byte-exact (a Lua string is bytes), and
  `res:save(path)` writes it to disk without it crossing Lua — `fs.write` takes UTF-8 and would
  reject those very bytes.
- An option a driver cannot honor is **refused**, never dropped — including `args` on `shell`,
  where the arguments belong in the argv itself. A refusal on an option you know exists means the
  binary under test is older than the proof.
- A protocol prova doesn't speak natively: drive the official CLI via `shell.run` argv, or
  wrap the SDK in a package (`prova learn package-authoring`).

See also: `prova learn doubles` (what the SUT calls out to) · `prova learn authoring` (the
assertions it feeds) · `prova learn capabilities` (what the host must have)
