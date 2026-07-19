# Mocks, Proxies, Drivers

Drafted 2026-07-19. Names the three roles Prova plays around a system under test, and the single
transport substrate they share. Supersedes the ad-hoc "mocking" framing that `examples/aspirational`
grew up under (that directory predates this model and is being subsumed). Builds on
[plugin-system.md](plugin-system.md), [namespacing.md](namespacing.md), and
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
observation model at once, and no plugin can assemble that from another plugin (see
[plugin-composition.md](plugin-composition.md)).

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
| `grpc`                | `grpc.mock`       | `grpc.proxy`                   | `grpc.call`                 | plugin |
| `socket` (uds)        | `socket.mock`     | `socket.proxy`                 | `socket.connect`            | kernel |
| `pipe` (named pipe)   | `pipe.mock`       | `pipe.proxy`                   | `pipe.connect`              | kernel |
| `process`             | —                 | `shell.proxy` (shim on PATH)   | `shell.run/spawn`           | kernel |
| **`terminal` (pty)**  | `terminal.mock`   | `terminal.proxy`               | `terminal.spawn` → session  | kernel |
| `postgres`/`redis`/…  | resource/container| capture/replay                 | native client               | plugin |

Two consequences worth stating:

1. **Transports self-declare their platform capability.** A `socket` on a Unix path *implicitly*
   folds `requires = { "unix" }` into the leaf; its Windows peer is a `pipe` transport that implies
   `windows`. Portable transports (`http`, `terminal`) work everywhere. Authors should not hand-write
   the platform `requires` for a transport that already knows its own platform.
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

Because every transport's Proxy wants this, the cassette **format, storage convention, matching
strategy** (how an inbound request/keystroke selects the recorded response), and **redaction** (scrub
secrets/timestamps at record time, or replays leak and diff-thrash) live **once in the kernel**, not
per transport.

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

## Kernel vs plugin

- **Kernel:** the transports whose *substrate* multiple plugins must share and therefore cannot live
  in any one plugin — `http`, `socket`/`pipe`, `process` (`shell`), and the new `terminal`; plus the
  two shared facilities Proxies introduce: the **cassette** engine and the **fault** vocabulary.
- **Plugin:** everything opinionated or dependency-specific — `grpc`/db/queue roles, framework-specific
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

## Status

- **Model:** settled (this doc).
- **Next:** `terminal` transport in the kernel (pty alloc + screen model + `expect`/`wait_stable`);
  the cassette engine; the fault vocabulary. Executable proofs pin the invariants first (red), per
  [proof-driven-development.md](proof-driven-development.md).
- **Cleanup:** re-point or retire `examples/aspirational` against this model; sweep comments that
  still say "mocking" generically.
