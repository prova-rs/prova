# The `stdio` transport, and the driver harmonization it forces

Drafted 2026-08-18. Closes `agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut`, and takes
the opportunity that backlog item exposes: the Driver posture is the one posture
[`mocks-proxies-drivers.md`](../design/mocks-proxies-drivers.md) never gave a shared contract, and
four kernel transports have been quietly inventing four dialects of it.

Builds on [mocks-proxies-drivers.md](../design/mocks-proxies-drivers.md) (the three postures),
[api-freeze.md](api-freeze.md) §3 (one matcher, N surfaces) and §6 (one journal spine).

## 1. The gap

`Process` (`shell.rs:279-341`) exposes `output()`, `running()`, `stop()`, `wait()` and no write
side — stdin is nulled at spawn (`shell.rs:849-851`). `shell.run{stdin=…}` and
`Container:run{stdin=…}` take one string, written before the program runs. So there is no way to
write to a live process and read the reply *before deciding what to write next*.

That rules out every SUT whose protocol is a conversation over stdio: MCP servers, LSP servers,
REPLs, debug adapters, interactive CLIs. Not a niche shape — prova ships an `mcp` mode of its own,
and MCP-over-stdio is how agents reach most tools now.

**Batching is not a workaround, it is a race.** Feeding a server its whole session on stdin at once
made it dispatch the calls concurrently: a `respond` reached the session lock before `render` had
stored anything and answered "No active render session." Red for a reason unrelated to the behavior
under test — and flaky, not red, had the scheduling gone the other way.

This repo is already living on that race. Every MCP proof it has drives `prova mcp` by pre-writing
`requests.jsonl` and redirecting it in — `proofs/mcp/surface_test.lua:26-43`,
`crates/prova-cli/selftest/mcp_warm_test.lua:40-54`. They are green only because prova's own MCP
server happens to answer sequentially. **Prova cannot currently prove its own primary agent
interface honestly with its own runtime.**

## 2. What the theme wants

api-freeze §7 reserved `Process:expect(pattern)` for this. **That line is retired by this plan.** It
was written 2026-07-15, before `terminal` and `socket` existed, and it predicted the wrong home:
`terminal:expect` shipping is what revealed that *conversation* belongs to a session type, not to a
process handle. `shell.spawn`'s nulled stdin is not a limitation to fix — it is the hermeticity
promise of "boot the app, probe it from outside." Making `Process` bimodal would spend that to save
a namespace.

Every kernel driver differs in exactly two dimensions. Conversation itself never varies:

| obtain the stream | namespace | observe it |
|---|---|---|
| dial an address | `socket` | raw bytes / framed turns |
| upgrade over http | `websocket` | message turns |
| spawn on a pty | `terminal` | screen |
| **spawn on pipes** | **`stdio`** | **framed turns** |

`stdio` and `terminal` are siblings differing only in pty allocation — which is exactly why they
cannot be one namespace with `pty = false`. `terminal`'s whole justification
(`mocks-proxies-drivers.md:100-104`) is that the screen *is* the observation layer; a pty-less
terminal has `:screen()`, `:cell()`, `:resize()` as nil-returning holes. An option that changes the
type is worse than two types.

Name: `stdio` over `pipe` (retired as a transport name when it folded into `socket`) and over `rpc`
(overpromises — it carries bytes, not RPC semantics). It is also what someone searching types.

## 3. The Session contract

Three verb families. **No transport may spell one of these ideas differently** — that is the rule
this plan adds, and `proofs/spec/cohesion/` holds it executably.

| family | verb | where it exists |
|---|---|---|
| **drive** | `:send(data)` | every session |
| **observe** | `:recv(opts?)` | where there are turns (framing set) |
| | `:expect(pattern, opts?)` | where there is a raw transcript to scan |
| **lifecycle** | `:close()` / `:stop()` | every session — one act, both grammars |
| | `:wait(opts?)` | where a process is owned |

`recv` and `expect` are not redundant: `recv` reads the *next turn*, `expect` scans *until the
stream shows something*. `where` on `recv` is the framed analogue of `expect`'s pattern —
`recv{ where = … }` is "the next turn that matches", skipping (and journaling) the rest.

Both are **bounded and loud**. Default 10s (socket's `DEFAULT_IO_TIMEOUT`), overridable per call.
The bounded read is the load-bearing half of this feature: an unbounded one turns a wedged SUT into
a hung suite, which is the failure `first_byte`/`idle_timeout` already exist to prevent for
`shell.run`.

Direction-tagged evidence is `:transcript()` — `{ seq, dir, data }`, the `wiretap::ProxyTranscript`
rows a proxy already produces. **The naming rule, stated once:** `received()` is the *listen*
postures (what arrived at a mock or a proxy's inbound side); `transcript()` is the two-directional
record. A driver has a transcript, never a journal.

## 4. `stdio` — the three postures

### Driver

```lua
local mcp = stdio.spawn(ctx, {
  cmd     = { prova.bin, "mcp" },   -- string | string[], exactly like shell.run/spawn
  framing = "line",                 -- MCP stdio is newline-delimited JSON
  codec   = "json",                 -- turns arrive decoded
  cwd = …, env = …,
})

mcp:send{ jsonrpc = "2.0", id = 1, method = "initialize", params = {…} }
local hello = mcp:recv{ where = { id = 1 } }
mcp:send{ jsonrpc = "2.0", method = "notifications/initialized" }

mcp:send{ jsonrpc = "2.0", id = 2, method = "tools/call", params = {…} }
local out = mcp:recv{ where = { id = 2 }, timeout = "30s" }   -- notifications in between are skipped

mcp:eof()                                     -- half-close stdin: "no more requests"
t:expect(mcp:wait()):equals(0)                -- and it exits cleanly
```

Surface: `:send` `:recv` `:expect` `:transcript` `:stderr` `:eof` `:wait` `:stop`/`:close`, field
`.pid`. Allocated through `ctx:manage` like every other resource — killed and reaped on scope exit,
LIFO, for free.

`:eof()` is a distinct act from `:stop()` and both earn their place: half-closing stdin is how you
prove "the server exits cleanly when its client goes away", which is a real MCP/LSP obligation.
`:stop()` kills. `:close()` aliases `:stop()` per the cohesion rule.

### The third stream

Sockets have two directions; a process has three. **stderr never enters the frame stream.** Folding
it in — which is what `Process` does today, one 64KB ring for both (`shell.rs:244-277`) — is
precisely what would make a JSON-RPC reader eat log lines as protocol garbage.

- stdout → framed, feeds `recv`/`expect`
- stderr → `sess:stderr()`, a bounded diagnostic tail (same 64KB, oldest dropped)
- **every `recv`/`expect` timeout message includes the stderr tail, the frame count, and the child's
  status.** "The server logged a stack trace and stopped answering" is the dominant failure and is
  currently invisible. `terminal.rs:315-320` is the message to copy — the best diagnostic in the
  tree, and it exists because an empty screen could not distinguish "never ran" from "ran and said
  nothing" from "spoke and we lost it".

### Mock — terminate

```lua
local fake = stdio.mock(ctx, { as = "some-mcp-server", framing = "line", codec = "json" })
fake:on{ method = "tools/list" }:reply{ result = { tools = {} } }
-- hand fake.env to whatever spawns the SUT
```

The SUT spawns an MCP server; you shadow that name on PATH with a scripted framed responder. This is
`terminal.mock` without the pty, and `shellproxy.rs`'s spool-through-the-filesystem journal
mechanism transfers directly (the shim is a different *process*, so the journal channel is the
filesystem, not memory). Unstubbed turns are LOUD. Fields `.env` / `.path`, `:received(filter)`,
`:stop()`.

### Proxy — interpose

```lua
local tap = stdio.proxy(ctx, {
  as = "some-mcp-server", upstream = "some-mcp-server",
  cassette = "proofs/cassettes/mcp.json", mode = "auto",
})
```

The MCP/LSP wiretap, **turn by turn**. This is the cell `shell.proxy` structurally cannot fill: its
turn is one whole invocation — argv + stdin → stdout + exit (`shellproxy.rs:9-12`) — so an
interleaved session collapses to a single opaque blob. Record a real server's session once, replay
credential-free forever, on the shared `cassette.rs` engine (a third `kind`, alongside `socket` and
`shell`). Speaks the shared fault vocabulary.

## 5. `codec` — the new dimension

```
framing : bytes → turns     "line" | { delimiter = … } | { length_prefixed = n } | "content_length"
codec   : turn  ↔ value     "bytes" (default) | "json"
```

This is the piece that makes the whole thing more than a byte pump. With `codec = "json"`:

- `:send(t)` encodes a table onto the wire
- `:recv()` returns a decoded value
- `:recv{ where = { id = 3 } }` matches with `engine::subset_mismatch` — **the same structural
  subset matcher as `:matches`, `:on`, and `received(filter)`**

api-freeze §3 made one matcher serve three surfaces; this makes it four, and it is what moves
MCP/LSP id-correlation into the kernel instead of into every proof's hand-rolled read-until loop.
`where` also accepts a function predicate, exactly as `modules::journal_keep` already defines it —
that function is reused verbatim, not reimplemented.

`codec` is a **session-and-turn** dimension, not an stdio one: it lands on `socket` and `websocket`
in the same change (§6), because both already have turn models and both already want `where`.

## 6. The harmonization

Today's drivers disagree in ways nothing justifies:

| surface | takes ctx | takes opts | observe | manages itself |
|---|---|---|---|---|
| `socket.connect(addr, opts)` | no | yes | `:recv` | **no — the fd leaks to GC** |
| `websocket.connect(url)` | no | **no** | `:recv` | **no** |
| `terminal.spawn(ctx, opts)` | yes | yes | `:expect` | yes |
| `stdio.spawn(ctx, opts)` | yes | yes | both | yes |

### What changes

1. **Every driver holding an OS resource takes `ctx` first and self-manages.**
   - `socket.connect(ctx, { addr = "tcp://…", framing = …, codec = … })`
   - `websocket.connect(ctx, { url = "ws://…", codec = … })`

   Every mock and proxy constructor already takes ctx and routes through the shared
   `modules::manage` (`modules.rs:119-134`). A driver holding a live fd carries the same obligation
   and was simply missed. Today a `socket.connect` in a loop leaks descriptors until GC — an
   inconsistency that is also a bug.

2. **`:recv(opts?)` gains `where`** on socket and websocket, same matcher, same semantics.

3. **`codec` is accepted wherever `framing` is** — `socket.connect`/`listen`/`mock`/`proxy`,
   `websocket.*`, `stdio.*`.

4. **`:transcript()` on every driver session**, via the existing `wiretap::ProxyTranscript` trait,
   so the evidence shape cannot drift per transport.

5. **`Framing::ContentLength`** — `Content-Length: N\r\n\r\n<body>`, which is LSP and DAP. Adding it
   also settles an outstanding lie: `mocks-proxies-drivers.md:90` advertises "or a Lua chunker" as a
   framing strategy and `Framing::parse` (`socket.rs:205-237`) implements no such thing. Ship
   `content_length`; **delete the chunker sentence** rather than leave a documented capability that
   does not exist. Calling into Lua from inside the async read loop is the wrong shape anyway.

### What deliberately does not change

**`shell.spawn` stays exactly as it is** — no ctx argument, no write side. The principled line is
that `shell` is the *command* namespace (run a thing, get its output) and the transport family is
the *conversation* namespace. `Process` is already managed by `ctx:manage(proc)` by convention, the
blast radius is every consumer that boots an app, and the win would be cosmetic. Flagged here as
considered-and-declined rather than left as an apparent oversight — overrule it deliberately or not
at all.

## 7. Migration: refuse with teaching

The rule this codebase already runs on, stated for this change: **anything retired must error naming
its replacement, never no-op.** The machinery exists at three layers and none of it needs building:

| layer | mechanism | where |
|---|---|---|
| manifest | `[requires] prova = ">= 0.25"`, read *before* schema validation | `manifest.rs:873-913` |
| manifest | `RETIRED_KEYS` tombstones — "a removal is a tombstone, not a deletion" | `manifest.rs:711-714` |
| Lua opts | `opts::Closed` + `Teaching` — refused key, paired with where the behavior lives | `opts.rs:31-116` |

The version gate is the load-bearing one for consumers, and it is already ordered correctly: a
manifest written for a newer prova fails the gate with "this suite requires prova X, and this is
prova Y" *before* the schema can say "unknown key", because "unknown field" is exactly the wrong
thing to say to someone whose real problem is an old binary.

**The positional changes need a shape check, not an opts gate.** `socket.connect(addr, …)` and
`websocket.connect(url)` change their *first argument*, which `Closed` cannot see. Detect it
directly — arg 1 is a string, not userdata — and teach:

```
socket.connect(ctx, { addr = "tcp://…" }): pass the test or fixture context first, and the
address as a named option, so the connection is closed with the scope
```

**No bridge, no alias.** `modules.rs:42-46` already settled the discriminator: a deprecation shim on
a surface with few consumers "is just a second name to keep working and a second thing to explain."
The repo keeps warn-once bridges where consumers are many (`manifest.rs:748+`, retiring at 1.0) and
takes clean cuts where they are few (the `parse`/`dump` → `decode`/`encode` cut). These drivers are
the few case; that is *why* this work is happening now.

The counterexample worth naming, because it is the failure mode of self-healing: the `ctx:tempdir()`
incident (`agent-ergonomics.md` §29) where "a behavioral change to a verb arrived indistinguishable
from no change at all." That did not heal — it rotted for forty minutes and nearly converted a
safety proof into the act it forbids. **Silence is what breaks organic evolution; a loud refusal is
what makes it work.**

## 8. Proof plan

Spec-first, flag-free, under `proofs/spec/`:

- **`proofs/spec/stdio/`** — the three postures. Driver: a real request/response conversation with
  ordering that a batch cannot produce (turn two depends on state opened in turn one — the exact
  assertion that motivated this). Bounded reads: a silent child fails loud with stderr in the
  message. `where`: an interleaved notification is skipped, not mistaken for the reply. `eof` →
  clean exit. Mock: PATH shadow, stubbed turns, loud on unstubbed. Proxy: record → replay with no
  upstream.
- **`proofs/spec/cohesion/`** — extend with the Session contract: every driver session speaks
  `send`/`recv`-or-`expect`/`close`, every one takes ctx and is torn down with its scope, `where`
  behaves identically across socket/websocket/stdio, `codec = "json"` round-trips on all three.
  Structural, so a fifth transport cannot drift.
- **`proofs/mcp/surface_test.lua`** — **convert off the batch file.** This is the dogfooding
  payoff and the mutation test for the whole feature: those proofs currently assume sequential
  dispatch, and after conversion they *assert* turn ordering instead.

Negative controls matter here more than usual: a `where` that matches everything, a timeout that
never fires, and a framing that silently passes bytes through would all leave green proofs proving
nothing. Each proof states what it would catch.

## 9. Teaching surface

Proof-guarded, and it can disagree with itself — `library/*.lua` and `skill.md` have drifted before
and caused a field bug. All of it in the same change:

- `RESERVED_NAMESPACES` (`lib.rs:59`) — add `stdio`; check the default-injection list.
- `library/modules.lua` — the `prova.Session` class, `stdio.*`, and the amended `socket`/`websocket`
  signatures. This is the source `prova.help()`, MCP `introspect`, and editor completion all read,
  so it is the surface that must not drift.
- `prova learn` — the transport topic; `docs/design/mocks-proxies-drivers.md` matrix gains the
  `stdio` row and loses the chunker claim.
- `crates/prova-cli/src/skill.md` — if it names driver signatures.
- `docs/plans/api-freeze.md` §7 — strike the `Process:expect` line with a pointer here.

## 10. Increments

Each lands green and is committable on its own.

1. **`codec` + `where` on the existing turn transports** (socket, websocket). No new namespace, and
   it proves the matcher reuse before anything depends on it.
2. **`Framing::ContentLength`**; delete the chunker claim.
3. **`stdio.spawn`** — the Driver, stderr separation, bounded reads with the diagnostic message.
4. **The ctx/self-management harmonization** on socket/websocket + the teaching refusals.
5. **`stdio.mock`** — the PATH-shadow responder.
6. **`stdio.proxy`** + the cassette `kind`.
7. **Convert `proofs/mcp/` and the selftest battery** off the batch file.
8. **Cohesion proofs + teaching surface**, closing the loop.

1–3 is the minimum that discharges the backlog item. 4 is the harmonization. 5–8 complete the row
and collect the dogfooding payoff.
