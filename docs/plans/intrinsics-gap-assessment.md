# Intrinsics Gap Assessment — the road to world-class black-box acceptance testing

Drafted 2026-07-23 from a full audit of the built surface (modules.rs, engine.rs, plugins,
testdata, proofs, selftest) against the design docs. Companion to
[kubernetes-stress-test.md](../design/kubernetes-stress-test.md) (one domain, deep) — this is the
holistic pass (every axis, ranked). Feeds [north-star-roadmap.md](../design/north-star-roadmap.md).

## The frame: what "complete" means here

Prova's promise is three roles around any SUT — **Drive** (originate), **Mock** (terminate),
**Ephemeral Dependencies** (provision) — plus **Proxy** (interpose) as the fourth posture that
record/replays and injures traffic, across a variety of technologies and I/O, **without
recompiling prova**. That last clause is the litmus test that organizes every gap below:

> **Plugins are pure Lua. Therefore anything that requires native code and is broadly needed
> must be a kernel intrinsic; everything composable from intrinsics belongs in plugin-land.**

A gap is *load-bearing* when its absence blocks a whole class of plugins (no pure-Lua plugin can
add TLS, a PTY, or SHA-256); it is *ergonomic* when a plugin could fake it but shouldn't have to;
it is *out of scope* when a Lua library or `shell.run` covers it honestly.

## 1. Naming: mocks vs doubles — already settled in code; write it down

The recurring debate is resolved by what already shipped, and the split is good:

- **`prova.double`** (built: `plugins/prova/double.lua`) — the **in-process, function-shaped**
  seam: mock / proxy / spy roles over a callable, `:on(match):reply(...)`, ordered `:received()`
  journal.
- **`X.mock`** (built: `http.mock`, `grpc.mock`) — the **out-of-process, transport-shaped** seam:
  a real listening server the SUT connects to.

Same grammar (`on/reply/received`), different seam. "Double" for function seams matches the
Meszaros umbrella term where it is actually apt; "mock" for network fakes matches the colloquial
dominant (WireMock, MockServer). **Decision to record in mocks-proxies-drivers.md:** keep both
terms with this exact split; never "double mocks." Add a two-line glossary mapping onto Meszaros
(his verifying "mock" = our proxy/spy posture; our Mock = his stub/fake) so the debate becomes a
footnote instead of a recurring agenda item.

## 2. The transport × posture matrix — as built (not as designed)

The audit corrected the design doc's own status section: **the http Proxy posture is largely
built** — `passthrough` (interpose, stub-wins partial mocking), `record`/`replay` **cassettes
ship today** (JSON, credential redaction by default, strict replay: unrecorded → 404, ordered
replay, drift-proof tested in `mock_proxy.lua`), and per-reply `delay` is latency injection.
mocks-proxies-drivers.md still lists cassettes as "next" — stale; update it.

| transport | Mock (terminate) | Proxy (interpose) | Driver (originate) |
|---|---|---|---|
| `http` | ✅ rich (routes, params, handlers, 501-on-unset) | ◐ passthrough + cassettes + delay; **no fault verbs, no matcher-gated interpose** | ✅ verbs + client + wait_for; **plaintext only, minimal options** |
| `grpc` | ✅ unary (protox compile, reflection served, error-code injection, delay) | ✗ | ✅ unary via reflection; **no streaming, no metadata, plaintext** |
| `graphql` | ✗ **absent** | ✗ | ✅ query/execute |
| `function` (in-process) | ✅ `prova.double` | ✅ (proxy/spy roles) | n/a |
| `process` | ✗ (PATH-shim shadow designed) | ✗ | ✅ `shell.run`/`spawn` |
| `socket`/`pipe` | ✗ | ✗ | ✗ (only `net.free_port`) |
| `terminal` (pty) | ✗ | ✗ | ✗ (designed in detail) |
| websocket / SSE | ✗ | ✗ | ✗ |
| resource (pg/redis/…) | via container plugins | ✗ (capture/replay designed) | ✅ sqlite native; rest docker-exec |

Strengths worth naming: the http mock's **honesty invariants** (501 on unset reply, 502 on dead
upstream, strict replay, handler-errors raised at `:stop`, journal-as-data instead of a verify
DSL) are a differentiated design — better defaults than WireMock's silent 404s. The
network-vantage story (`network=true` → host-gateway + unconditional `extra_hosts`) is built and
tested.

## 3. Gaps, ranked by the litmus test

### Tier A — kernel intrinsics (native-code-required, class-blocking)

1. **Subset/structural matcher + table diff** (`engine.rs`). `equals` is strict, `contains` is
   flat, `display()` renders any table as `<table>`. Blocks: K8s plugin (its differentiator),
   every JSON-API assertion, `prova.double`'s own `:on` subset matching (currently reimplements
   it in Lua — converge them). Smallest, highest-leverage item on this list.
2. **Encoders: `json.encode`, `yaml.dump`** (`modules.rs`). Verified: **no Lua-callable encoder
   for any format exists**; proofs never round-trip (decode-and-assert only), which masks it.
   Blocks: table-first manifest authoring, request-body building beyond `json=`, cassette
   post-processing, any plugin that must *produce* structured text.
3. **TLS — client first, mock second.** Deferred by design in v1, but it is the first wall an
   adopter hits: you cannot Drive a real staging API over https, attach to a secured dependency,
   or mock an https-only SDK. Client (`https` via rustls feature) unblocks most; TLS on
   `http.mock` follows for SUTs that refuse plaintext.
4. **Stub request matchers on mocks: query / header / body(-subset)** — matching is
   method/path/route only; handlers can inspect but not *select*, and `received()` filters only
   method/path. This is the gap between "a mock" and "a contract-grade mock." Body matching
   should reuse the same subset matcher as #1 — one semantics, three surfaces (assertions,
   doubles, mock stubs).
5. **Fault vocabulary on the interpose path** — `latency/drop/corrupt/throttle/after` (designed,
   shared kernel facility). Today only `delay` + status injection. This is the "prove resilience,
   not happy paths" pillar; toxiproxy-in-process is a headline capability no pure-Lua plugin can
   add.
6. **Streaming: SSE + chunked on http (mock and client), gRPC server-streaming + metadata.**
   Everything today is fully buffered, unary, metadata-blind. Modern APIs (LLM SSE endpoints,
   watch streams, grpc streaming) are untestable. Stage: gRPC metadata (small) → SSE mock/client
   → grpc server-streaming → ws/bidi later.
7. **`socket` transport (raw TCP first)** — mock/proxy/connect. Unlocks the *generic* interpose
   posture for any wire protocol (the `socket.proxy` from the design doc's own example) and is
   the substrate for resource capture/replay. UDS/named-pipe variants follow the platform-gating
   design.
8. **`terminal` (pty) transport** — fully designed, zero code. The declared differentiator vs
   all prior art; `shell.spawn` is pipe-based and cannot honestly test a TUI/wizard. Also
   subsumes the streaming-`:expect` need from the K8s audit.
9. **On-failure diagnostic attachments** (`model.rs` + reporters + engine interception) — from
   the K8s audit; every resource plugin wants "show the logs when the assert fails." Today: one
   flat message string; `ctx:log` goes to stderr, not the event stream.
10. **Utility belt (tiny, native-required):** `base64`, `sha256`/`hmac`, `uuid`, a `url`
    parse/encode module (dep already in-tree, unexposed), `prova.parse.csv`. Each is a
    ten-line binding; collectively they prevent the `shell.run("openssl …")` workarounds that
    haven't appeared in proofs *only because the proofs sidestep them.* Explicitly **not**
    included: regex (Lua patterns + `string.match` cover the honest cases), templating (Lua
    string interp), compression (shell out), TOML-to-Lua (no demonstrated need — revisit on
    demand).

### Tier B — Driver ergonomics (buildable today, shouldn't have to be)

- http client: `form=` (urlencoded), `multipart=`, `auth={basic=/bearer=}`, cookie jar opt,
  redirect policy opt, per-client reqwest reuse (currently rebuilt per call).
- `graphql.mock` — the one missing first-party mock; buildable *in Lua on top of `http.mock`*
  once body matchers (#4) exist — a good dogfood of the tier system.
- `prova.double` ↔ kernel matcher convergence (see #1).

### Tier C — explicitly plugin-land (resist the pull)

JWT DSLs, data builders/fakers, XML/HTML parsing, SMTP, cloud-provider anything, k8s. The
ecosystem doc's discipline holds: these compose from intrinsics. The kernel's job is to make
them *possible*, not to absorb them.

## 4. Test-coverage gaps (the "not adequately tested" half)

The harness core is in genuinely good shape — fixtures/scopes, resources/DAG, docker (incl.
failure diagnosis + readiness timeout), plugins (searcher/git/private-deps/lint), reporters
(JUnit/TAP/JSONL content-verified in selftest), MCP (cold+warm), init/ide/eval all rate **good**.
The soft spots, ranked:

1. **`prova watch` — zero tests of any kind** (`main.rs:884` dispatched, never invoked by a
   test). An advertised workflow verb with unproven supervise/re-run behavior.
2. **`--last-failed` — no behavioral test.** Nothing writes a failure set then proves the rerun
   selects exactly it. This is the PDD loop's own crank — it deserves a selftest.
3. **`graphql` — one Rust test, no testdata, no proof, no example.** Variables/headers untested.
4. **`sqlite` — happy-path only**; no error paths (bad SQL, constraint violation), no tx.
5. **`yaml` / `prova.parse` / `net` — happy-path only**; malformed-input behavior unpinned.
6. **`grpc.mock` — unit-only**; no proof-layer dogfood (http.mock has one; parity suggests one).
7. **Port-collision recovery — trigger never proven from Lua**; counters only move under
   Rust-injected faults.
8. **Reporter edge shapes** — error-vs-failure outcome, skip XML, escaping via the CLI path.
9. **retry × manage interaction** — retry exhausting *inside* a `ctx:manage` provisioning and
   the teardown consequence; the two suites never intersect.
10. **Housekeeping finding:** `../prova-mocks` and `../prova-agents` are byte-identical mirrors
    of `prova/` (jj workspaces, shared store) — they add zero coverage; all real coverage lives
    in this tree. Don't let the mirrors read as breadth.

Also: `examples/aspirational` cleanup and the "sweep comments that still say 'mocking'
generically" item from mocks-proxies-drivers.md remain outstanding.

## 5. Recommended sequence

Interleave one *capability* track and one *trust* track — each capability lands proof-first (red
suites pin the invariants, per proof-driven-development.md), and each trust item hardens what
already ships.

| # | Capability track | Trust track (parallel) |
|---|---|---|
| 1 | Subset matcher + table diff (one semantics for expect/double/mock-stub) | `--last-failed` behavioral selftest |
| 2 | `json.encode` / `yaml.dump` + utility belt (base64/hash/uuid/url/csv) | `prova watch` test harness |
| 3 | Mock stub matchers (query/header/body via #1) → `graphql.mock` in Lua as dogfood | graphql + sqlite error-path suites |
| 4 | TLS client (`https` feature) | yaml/parse/net malformed-input pins |
| 5 | Fault vocabulary on passthrough (`drop/corrupt/throttle/after`) | grpc.mock proof-layer dogfood |
| 6 | gRPC metadata + SSE, then grpc streaming | reporter edge shapes |
| 7 | `socket` transport (tcp mock/proxy/connect) | retry × manage intersection |
| 8 | On-failure diagnostic attachments | port-collision black-box trigger |
| 9 | `terminal` transport (the differentiator, biggest single build) | — |

Items 1–3 are the compounding core: the subset matcher powers assertions *and* doubles *and*
mock matching; encoders unblock authoring; together they make the K8s plugin, `graphql.mock`,
and contract-grade http mocking all fall out of the same two intrinsics. TLS (4) is the
adoption wall. Faults/streaming/socket (5–7) complete the resilience story. Terminal (9) is the
moat and can proceed independently whenever appetite allows.

## 6. Doc corrections to fold in (drift found by the audit)

- mocks-proxies-drivers.md: cassettes/passthrough/delay are **built** (status says "next");
  add the `double` seam to the transport table (`function` row) and the naming glossary (§1).
- ecosystem.md/api.md: document `prova.double` as a first-party library plugin — it is invisible
  in the design docs today despite full proof coverage.
