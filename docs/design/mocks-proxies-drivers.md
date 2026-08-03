# Mocks, Proxies, Drivers

Drafted 2026-07-19. Names the three roles Prova plays around a system under test, and the single
transport substrate they share. Supersedes the ad-hoc "mocking" framing that `examples/aspirational`
grew up under (that directory predates this model and is being subsumed). Builds on
[package-system.md](package-system.md), [namespacing.md](namespacing.md), and
[topologies.md](topologies.md).

## The insight this is built on

Everything Prova does to a SUT is one of three **postures on a stream**. We already ship the seed of
this: `http` carries both a fake server (`http.mock`) and a client (`http.get`); `shell` drives a
process (`shell.run`/`shell.spawn`). The model just makes the pattern explicit so every new transport
slots in the same way.

A **transport** (http, socket, terminal, process, grpc…) is a thing that can **listen**,
**connect-or-spawn**, carry bytes, be **observed** through a transport-native model (an HTTP
request/response, a terminal screen, raw frames), and be **torn down** on scope exit. The three roles
are three postures on that one substrate:

| role       | posture       | topology                                              |
|------------|---------------|-------------------------------------------------------|
| **Mock**   | **terminate** | listen, answer synthetically — no upstream            |
| **Proxy**  | **interpose** | listen *and* connect upstream — sit in the middle     |
| **Driver** | **originate** | connect/spawn + observe — you are the traffic         |

Mocks and Proxies are the *world around the SUT* (they `listen`); Drivers are the *SUT side* (they
`originate`). The Proxy is the only one that touches both — which is exactly why it is the most
powerful and the most kernel-bound: it needs the listen substrate, the client substrate, and the
observation model at once, and no package can assemble that from another package (see
[package-composition.md](package-composition.md)).

## The three roles

**Mock — terminate.** Stand in *place of* a dependency the SUT calls. Fully synthetic responses; no
real upstream. This is the shipped `http.mock`.

```lua
local m = http.mock(t)
m:on{ path = "/ping" }:reply{ status = 200, body = "pong" }
-- point the SUT (or a Driver) at m.url
```

**Proxy — interpose.** Sit *between* the SUT and a real dependency (or a Mock). Traffic flows
through, and the Proxy may spy on it, assert on it, record it, replay it, or injure it
(latency/faults). A Proxy in record mode against a real dependency **manufactures a Mock** (a
cassette) — which is Prova's whole ethos in one mechanism: prove against reality once, then pin it
deterministically forever.

```lua
local db = socket.proxy(t, { upstream = pg.addr })   -- interpose on a real dependency
db:latency("300ms"); db:after("2s"):drop()           -- prove resilience, not just happy paths
t:expect(db:transcript()):contains("BEGIN")          -- spy: assert on what actually flowed
```

**Driver — originate.** Act on and observe the SUT itself. The shipped `shell.run`/`shell.spawn` and
`http.get`/`http.wait_for` are Drivers. `terminal.spawn` (below) is the new one.

Note that a Driver's readiness gate (`http.wait_for`) and a terminal Driver's `:expect(pattern)` are
the *same idea* — "block until observed state matches, with a timeout." We standardize that
vocabulary across Drivers: `wait_for` / `expect`, never a sleep.

## One transport vocabulary

A Mock's endpoint and a Driver's target are the **same value** — a Mock exposes `.url`/`.addr`/
`.endpoint`, and the matching Driver verb consumes it — so "point the real client at the fake" is the
default, not a special case. Every transport advertises the same three verbs where they make sense:

| transport             | Mock (terminate)  | Proxy (interpose)              | Driver (originate)          | layer  |
|-----------------------|-------------------|--------------------------------|-----------------------------|--------|
| `http`                | `http.mock`       | `http.proxy`                   | `http.get/post/wait_for`    | kernel |
| `grpc`                | `grpc.mock`       | `grpc.proxy`                   | `grpc.call`                 | package |
| `socket` (tcp + uds)  | `socket.mock`     | `socket.proxy`                 | `socket.connect/listen`     | kernel |
| `websocket`           | `websocket.mock`  | `websocket.proxy`              | `websocket.connect`         | kernel |
| `process`             | —                 | `shell.proxy` (shim on PATH)   | `shell.run/spawn`           | kernel |
| **`terminal` (pty)**  | `terminal.mock`   | `terminal.proxy`               | `terminal.spawn` → session  | kernel |
| `postgres`/`redis`/…  | resource/container| capture/replay                 | native client               | package |

Two consequences worth stating:

1. **One `socket` namespace, unified by address scheme** (decision 2026-07-27, supersedes the
   separate `pipe` transport): `tcp://host:port` and `unix:///path` — future `npipe://` on Windows —
   are just addresses. Listen, connect, proxy, and the byte model are identical across schemes; only
   address parsing differs. **Transports still self-declare their platform capability**: a `unix://`
   address *implicitly* folds `requires = { "unix" }` into the leaf; portable transports (`http`,
   `terminal`) work everywhere. Authors should not hand-write the platform `requires` for a
   transport that already knows its own platform. And because a raw byte stream has no natural
   "request" unit, mocks and transcripts take a **framing** strategy (`"line"`,
   `{ length_prefixed = n }`, `{ delimiter = "…" }`, or a Lua chunker) to turn bytes into matchable
   turns — which is also what makes the byte-level `socket.proxy` the universal wiretap: put it in
   front of Postgres, Redis, Kafka, anything TCP, and you get transcripts and fault injection with
   zero protocol knowledge.
2. **Prior art we are deliberately converging on:** mountebank (multi-protocol imposters that are both
   stubs and proxies), toxiproxy (the fault vocabulary), WireMock/VCR (record-replay). What none of
   them have is the terminal transport or Prova's capability-gated cross-platform proof story — that
   is the differentiated part.

## The terminal transport (the worked new example)

`terminal` is Driver-primary and belongs in the **kernel** — it is the PTY-backed sibling of
`shell.spawn`. The decisive reason it is one kernel API and not two per-OS ones: **only the
allocation differs by platform** (openpty on Unix, ConPTY on Windows, both behind `portable-pty`);
ConPTY emits the same VT sequences openpty does, so the **screen model — the observation layer — is
byte-for-byte OS-agnostic.**

```lua
prova.test("wizard confirms on the alt screen", function(t)
  local term = terminal.spawn(t, { cmd = { "./myapp", "init" }, cols = 80, rows = 24 })

  term:expect("Project name:")             -- block until stream/screen matches (timeout'd)
  term:send("acme\r")
  term:wait_stable()                        -- settle the frame; never sleep

  local s = term:screen()                   -- the observation type
  t:expect(s:contains("Create 'acme'? (y/N)")):is_true()
  t:expect(s:cell(0, 0).fg):equals("red")   -- styled-cell assertions
  t:expect(s):matches_snapshot("confirm")   -- golden frame

  term:resize(120, 40)                       -- SIGWINCH; prove reflow
  term:signal("INT")                         -- prove clean Ctrl-C teardown
end)
```

- **Session surface:** `:send`, `:expect`, `:wait_stable`, `:screen`, `:resize`, `:signal`, `:wait`.
- **`Screen` type:** `:text`, `:line(n)`, `:cell(r,c)` (char + fg/bg/attrs), `:contains`,
  `:matches_snapshot`.
- **Lifecycle:** allocated via `ctx:manage` like any resource — the child is killed and the pty
  restored on scope exit, LIFO, for free.

The **terminal Mock** is the narrow, true "mock": your SUT shells out to an interactive CLI (`ssh`,
`psql`, an installer), and you shadow it on `PATH` with a scripted responder built on the same kernel
pty primitive.

```lua
local ssh = terminal.mock(t, { as = "ssh" })          -- shadows `ssh` on PATH
ssh:expect("password:"):send("hunter2\n")
```

## Cassettes (shared kernel facility)

A **cassette** is a *recording*, not a hand-authored script — the transcript a Proxy captures in
record mode and replays later. For terminal it carries frame timing (asciinema-shaped); for http it
is request-key→response (VCR-shaped); the lifecycle is identical. It is a Mock you did not have to
write, and it is human-editable after capture.

```lua
local psql = terminal.proxy(t, {
  as = "psql", upstream = "psql",
  cassette = "proofs/cassettes/seed.cast",
  mode = "auto",            -- record if the cassette is absent, else replay
})
```

Because every transport's Proxy wants this, the cassette **format, storage convention, modes**
(`record` / `replay` / `auto` / `passthrough`), **matching strategy** (how an inbound
request/keystroke selects the recorded response — a replay miss is always LOUD), and **redaction**
(scrub secrets/timestamps at record time, or replays leak and diff-thrash) live **once in the
kernel**, not per transport. Each transport contributes only its **turn model** (http/grpc:
request→response pairs; shell shim: argv+stdin→stdout+exit; socket: framed turns; terminal: timed
frames) and its **match key**.

One honest limitation, by design: VCR semantics hold on request/response transports; **full-duplex
transports** (raw socket, websocket, terminal) replay as a **scripted conversation** — ordered,
expectation-driven, timing-annotated. Same file format, different replay discipline. Don't promise
VCR semantics for streams; promise the conversation model.

## Fault injection (shared vocabulary)

The interpose posture is the only one that can prove resilience rather than the happy path. A single
vocabulary — `latency`, `drop`, `corrupt`, `throttle`, `after` — lives on the proxy substrate and any
stream transport applies it. No extra daemon (toxiproxy in-process).

## Capability & platform gating

Platform gating is *already solved* by the capability system: `unix` and `windows` are built-in
capabilities (`cfg!(unix)`/`cfg!(windows)`), with a `must_run` counterpart in `prova.toml` that turns
a silent skip into a hard failure. Proving Prova-on-Windows behaves is therefore two proofs plus a
`must_run`, with **no new mechanism**:

```lua
prova.test("reflow on resize — unix pty", { requires = { "unix" } },    function(t) ... end)
prova.test("reflow on resize — ConPTY",   { requires = { "windows" } }, function(t) ... end)
```

```toml
# prova.toml on the Windows CI runner — a windows-gated test that SKIPS here is now a FAILURE
[context]
must_run = ["windows"]
```

Record-replay makes this cheap to keep green: record a ConPTY cassette on the Windows runner, commit
it, and every other platform replays it deterministically without a Windows box.

## Kernel vs package

- **Kernel:** the transports whose *substrate* multiple packages must share and therefore cannot live
  in any one package — `http`, `socket`/`pipe`, `process` (`shell`), and the new `terminal`; plus the
  two shared facilities Proxies introduce: the **cassette** engine and the **fault** vocabulary.
- **Package:** everything opinionated or dependency-specific — `grpc`/db/queue roles, framework-specific
  TUI helpers, and the `terminal.mock` conveniences — *because* the raw `terminal` primitive is in the
  kernel for them to build on.

## Naming decisions

- **Mock / Proxy / Driver** — keep **Mock** over "Double." It is the colloquial dominant (WireMock,
  mockito, mockserver) and `.mock` already ships. Caveat kept in mind: purist (Meszaros) taxonomy
  calls a *verifying* double a "mock" — which in our model is the **Proxy/spy**. Prova draws the line
  at *behavior* (Mock = terminate, Proxy = interpose) rather than at verify-or-not, which is a cleaner
  cut.
- **`terminal`** over `pty` for the user-facing word (reads as intent); `pty` stays the internal
  kernel module name.

## Non-goals

- **Browser** — Playwright's job. **Device input** — Minion's job.
- **Clock/time** — a real port of a system but not a transport; the black-box answers (env
  overrides, through-app time endpoints, container clocks) are recipes, not a primitive.
- **Filesystem doubling** (a FUSE-level fake fs) — `fs` driving plus temp-dir fixtures cover the
  black-box story; a fake filesystem is a white-box tool.
- **Webhooks/callbacks** are not a missing transport: "the SUT calls *me* back" is `http.mock` plus
  waiting on `:received` — a documented recipe ("callback capture"), not a feature gap.

## Status

- **Model:** settled (this doc).
- **SHIPPED (2026-07-27):** the whole Tier-A surface is implemented and guarded by flag-free
  proofs under `proofs/spec/` — `socket` (scheme-unified tcp+uds, framing, all three postures,
  direction-tagged transcripts), the fault vocabulary (latency/drop/corrupt/throttle/after on the
  proxy substrate; http.proxy speaks latency/drop/after and points at socket.proxy for byte-level
  faults), cassettes (modes passthrough/record/replay/auto on `http.proxy` — sugar over the mock
  dial; loud 502 replay miss; record-time redaction), `terminal` (portable-pty driver, vt100
  screen model, golden frames via the snapshot protocol, PATH-shadow mock), `websocket`
  (full-duplex, on_connect push, and `websocket.proxy` — the last cell in the matrix), and
  `shell.proxy` (the journaling PATH shim). Every journal speaks the §6 seq/source/matched spine;
  every new namespace is in the §2 reserved registry and the `library/modules.lua` stubs.
- **Cassettes, generalized (2026-07-27):** record/replay spans every transport — `http.proxy` (the
  first), `grpc.proxy` (self-describing: the cassette carries the reflected descriptors, so replay
  needs no proto and no upstream; a miss is a loud `Unavailable`), `socket.proxy` (framed-turn VCR),
  `shell.proxy` (record a real CLI once — argv+stdin → stdout+exit — replay credential-free), and
  `terminal.proxy` (the asciinema-shaped session cassette — VT sequences intact, the cross-platform
  ConPTY-replay mechanism). Redaction is a kernel facility, not http-only: `redact = { strings }`
  scrubs literal secrets from the serialized cassette at record time on every transport.
- **Resource tapping (2026-07-27):** `prova.containerized` grew a `tap` option — `X.container(ctx,
  { tap = true })` interposes a `socket.proxy` between the SUT and the real container, routing
  `res.url` through it and exposing `res.tap`, so any containerized dependency gets transcripts and
  fault injection with zero protocol knowledge. The recipe author declares the wire `framing` once;
  turn-level taps follow for free.
- **The matrix is COMPLETE (2026-07-27):** every transport has all three postures. `terminal.proxy`
  closed the last cell (terminal's interpose + record/replay). The cohesion seams are closed too:
  a universal `.endpoint` (the "same value" promise made literal across `.url`/`.addr`), `:close()`
  on every proxy, and `grpc.proxy` speaking the shared fault vocabulary (latency/drop/after).
- **Still open — by design or platform, not a hole in the matrix:** ConPTY (Windows) twins for
  terminal/shell.proxy behind a Windows runner + `must_run` (see
  `docs/plans/windows-lane-via-parallels.md` — a local Parallels guest is the planned first
  vantage); timing-annotated input matching for the full-duplex cassettes (v1 records output
  frames); TLS (and eventual MITM taps) on socket; the datagram/UDP port class (a new surface, a
  separate decision).
- **Cleanup:** re-point or retire `examples/aspirational` against this model; sweep comments that
  still say "mocking" generically.
