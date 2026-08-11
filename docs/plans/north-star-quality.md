# The north-star quality arc

Prova must be the exemplar of a prova-managed project. The tool computes every debt class this
plan addresses; this arc drives each one to its floor and leaves the *mechanism* — not a document —
holding the bar afterward.

**The plan's authoritative copy is not this file.** Phase 0 encoded it as `goal`s in
`.prova/baselines/quality.json`, where `prova run quality` enforces it: a reached goal FAILS the
run until the gain is banked and the goal retired or lowered. This document carries the parts a
baseline cannot: the ordering argument and the Phase 1 design. Fold what lands into
`docs/design/` and delete sections here as they complete.

## Where the arc started (2026-08-10)

Retired already: giant files (3 → 0, hard limit at 1500), production unwraps (20 → 0),
oversized functions 27 → 16, clones 31 → 19 (production-only scan), the profraw litter class.

Encoded goals: `functions.too_long → 0` · `duplication.clones → 8` · `expect.production → 0` ·
`coverage.unit → 70` (first step-goal; merged bar already 81).

## Where the arc CLOSED (2026-08-11) — every encoded goal reached and retired

Every debt class sits at its floor with the ratchet holding it: files 0 · functions 0 ·
unwraps 0 · expects 0 (two documented `#[allow]`s with their arguments in place) · clones 0.
`coverage.unit` crossed its 70 goal and banked at **73.46** (from 60.22); the goal is retired
because the plan's own exit criterion — the unit-owed delta list empty of >40-point gaps — is
met. The merged bar banked at 84.69, blackbox at 68.99 (its honest number after the
denominator fix below). The floors are mechanism now; raise a `goal` again when the next
paydown is chosen, and this document has nothing left to order.

What Phase 3 proved along the way, beyond the numbers:

- **Test-writing found real defects**: cassette encode's b64-sentinel spoofing, and `Counts`
  refusing schema-1 run records (which silently voided `Executed`'s `spec`-alias back-compat).
- **The coverage conduct needed two corrections its own ratchets caught**: banking per batch
  exposed the black-box layer paying denominator rent for nextest's test binaries (8.2 points;
  fixed by suite-first ordering + staging executables out of the scan), and run-to-run wobble
  motivated the `tolerance` field (declared noise bands on blackbox 1.0 / lines 0.25).
- **The duplication ratchet caught the tests themselves cloning** — the fix was a shared
  scaffold, not a looser baseline. The gates gate their own arc's work; that is the exemplar
  property this arc exists to demonstrate.
- **The last mile was harnesses, not helpers**: websocket/terminal/socket unit coverage came
  from driving each transport's own Lua surface (mock + driver + proxy) under a `LocalSet`
  in-process — loopback sockets and a real PTY, tens of milliseconds per test.

## The ordering argument: why unit tests come LAST

The unit-owed worklist (the layered coverage conduct's delta report) and the clone census point
at the SAME files: socket, websocket, terminal, shellproxy, cassette, the mock family, the cmd_*
verbs. Unit tests written against duplicated or 120-line code get written twice and then
invalidated by the extraction that was always coming. So:

1. **Phase 1 — extract the shared transport spine** (the clone paydown). The 80–94% black-box
   coverage on exactly these files is the safety net: behavior is held by proofs while code moves.
2. **Phase 2 — finish the function decomposition** (16 → 0), prioritized by the unit-owed list,
   `expect` sites fixed per touched file (broker.rs holds 10 of the 25). At zero, the
   count-ratchet becomes the hard limit, as with file-size.
3. **Phase 3 — the unit-test worklist**, now against stable, pure seams: `baselines.rs` /
   `measure.rs` / `cassette.rs` first (pure logic; the quality mechanism itself deserves unit
   proof), then the extracted spine, then the cmd_* helpers. Bank `coverage.unit` per milestone,
   re-aim the goal upward until the delta list is empty of >40-point gaps.

## Phase 1 design: the shared transport spine — LANDED 2026-08-10, clones 31 → 0

The census is ZERO and the `duplication.clones` goal is retired at its floor. The design below
executed as written, with two implementation-level judgments: the endpoint seam's honest shared
piece was the `.network` table (a function — `url`/`endpoint` semantics differ per transport, so
a trait would have forced them), and the trait impls themselves are macro-stamped
(`impl_journal!`/`impl_transcript!`/`impl_shutdown!`) because their bodies are identical for
every `Rc<RefCell<State>>`-shaped transport. The grpc reflection drain folded into its existing
`reflection_ops!` macro as a stamped `$drain_fn`. The durable home is `modules/wiretap.rs`'s own
docs; this section stays only as the record of what shipped.

### The original design (as reviewed and approved)

Every cross-module clone pair is a copy of one of six seams. The extraction target for each is a
small shared helper in `modules/` — trait-parameterized where a UserData registration is shared,
a plain function where the copy is a plain function. No transport's Lua surface changes; the
black-box suite is the referee.

### 1. The §6 wiretap surface — socket ↔ websocket (3 pairs + 1 internal)

Both transports hand-copy the journal exposure (`received(filter)` building
`{seq, data, matched, source}` rows through `journal_keep`) and the shutdown pair
(`stop`/`close` draining a oneshot). Extract `modules/wiretap.rs`:

```rust
pub(super) struct JournalRow<'a> { pub data: &'a [u8], pub matched: bool, pub source: &'static str }

pub(super) trait Wiretap {
    fn journal_rows(&self) -> Vec<JournalRow<'_>>;
    fn take_shutdown(&self) -> Option<oneshot::Sender<()>>;
}

pub(super) fn add_wiretap_methods<T, M>(methods: &mut M)
where T: Wiretap + 'static, M: UserDataMethods<T>
// registers received/stop/close once, for every transport that implements Wiretap
```

`socket::ProxyUd`/`MockUd` and `websocket`'s two UD types implement `Wiretap`; their
`add_methods` call the shared registration. This is also §6's contract made structural: a new
transport implements the trait and cannot drift the journal shape.

### 2. The mock endpoint surface — grpc_mock ↔ mock (2 pairs)

Both mock servers expose `url`/`addr`/`network` fields the same way (the network table mirroring
a container resource's, host-gateway flavored). Extract a `MockEndpoint` trait + shared
`add_endpoint_fields` into `modules/mock_endpoint.rs` (or fold into `wiretap.rs` if the file
stays small — judged at implementation).

### 3. The verifier ingestion seam — junit ↔ sarif (2 pairs)

`resolve_files(pattern, cwd)` (literal-or-glob expansion) is byte-identical but for the error
prefix, and the load-entry iteration matches. Extract into `modules/ingest.rs` with the verb name
as a parameter. This is the deputed-verifier seam: the third verifier (there will be one) should
cost a parser, not another copy of the plumbing.

### 4. The shim handle surface — shellproxy ↔ terminal (1 pair)

Both PATH-shim proxies expose `env`/`path` fields identically. Same trait-helper shape as #2;
likely lands in the same shared file.

### 5. `manage` — socket ↔ terminal (1 pair)

An identical free function tying a resource to `ctx:manage`. One copy moves to `modules.rs`
(the parent) as `pub(super) fn manage(what, ctx, ud)`; audit the other transports for private
variants of the same function while there.

### 6. Client construction — graphql ↔ http (1 pair)

`url`/`headers`/`timeout` option parsing. A shared `pub(super) fn client_opts(opts) ->
mlua::Result<(String, Vec<(String,String)>, Option<Duration>)>` in `modules.rs` or a small
`modules/client.rs`.

### Intra-file pairs (local dedup, no design needed)

- `cmd_topo.rs` ×2: up/watch/start share an arg-parsing prologue → one local parse helper
  (the dispatcher's `Cli` pattern, in miniature).
- `engine.rs` ×2: the reminders/census verbs share the suite-load boilerplate → one local
  `load_suite_collections` helper.
- `engine/discover.rs`, `engine/topology.rs`, `mcp/blocking.rs`: one small local helper each.
- `grpc.rs` (reflection request/response drain ×2): may be an honest residual — the two drains
  differ in request type and folding; extract only if it stays readable.

### What "goal 8" means

The eleven cross-module pairs die (spine) and most intra-file ones fold (local helpers); the goal
leaves room for honest residuals like the grpc reflection drains. When the census lands at/below
8, the ratchet fails demanding the goal be banked and re-aimed — decide then whether the
residuals are debt or idiom.

## Phase 3 note: what the spine buys the tests — CONFIRMED 2026-08-11

After Phase 1, the wiretap surface, endpoint fields, `resolve_files`, and `client_opts` are each
ONE unit — testable with plain values, no sockets, no docker. That, plus the pure-logic trio
(`baselines.rs`, `measure.rs`, `cassette.rs`), was indeed the bulk of the 60 → 70 climb; the
final points past 70 came from in-process transport harnesses (see the closing record above).
