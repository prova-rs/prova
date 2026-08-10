---
title: Placement — resolving capability and contention against a broker
type: design
maturity: design
tags: [design, placement, broker, leases, distribution, open-core]
---

# Placement

> A proof file is byte-identical whether it runs on one machine or forty.

Prova resolves two declarations before and around every unit of work:

- `requires = { "xcodebuild" }` — is this capability available? If not, **skip**.
- `resources = { prova.writes("window-server") }` — hold this exclusively while I run.

Today both are answered locally: `requires` probes `PATH`, `resources` is an in-process semaphore.
Both answers are correct and complete for one machine, and that remains the default forever.

**Placement widens where those questions are asked.** A *broker* — an optional, out-of-process
peer that prova dials over a Unix socket — can answer them across a pool of machines. `requires`
becomes a node selector; `resources` becomes a lease on a node-owned slot. Nothing about how a
proof is written changes, which is the point: you do not fork a suite to distribute it, and CI
keeps working as a pool of one.

## The seam

```
┌── prova (this repo, MIT) ─────────┐      ┌── broker (any implementation) ──────┐
│  requires  →  resolve             │ unix │  local: PATH probe + semaphore      │
│  resources →  claim/renew/release ├─────▶│  clustered: membership, trust,      │
│  placement client · reference     │ JSON │  gossip, cross-node placement,      │
│  local broker · this spec suite   │      │  workspace materialization          │
└───────────────────────────────────┘      └─────────────────────────────────────┘
```

Two protocols, two licences, and the line between them is deliberate:

| Protocol | Between | Licence |
|---|---|---|
| **Placement** (this document) | prova ↔ its **local** broker | MIT, specified, conformance-tested |
| Mesh | broker ↔ broker: discovery, pairing, gossip, inventory | implementation's own |

**Prova never dials a remote broker.** It speaks only to the one on its own machine; every
question of *which* machine, *whose* machine, and *how to reach* it lives behind the socket. That
is what keeps this protocol small, keeps TLS and discovery out of prova entirely, and lets the
reference local broker implement `exec` as `std::process::Command` while a clustered broker
implements it as a hop.

## Transport

Newline-delimited JSON over a Unix domain socket — `socket.connect(addr, { delimiter = "\n" })`
in prova's own vocabulary. No TLS: a Unix socket is protected by filesystem permissions, and
there is no remote peer on this hop to authenticate.

<!-- claim: broker-address-resolution -->
Address resolution, first match wins (a blank value is unset, so an empty env var disables rather
than misdials):

1. `PROVA_PLACEMENT_BROKER` — a `unix://` address
2. `[placement] broker = "unix://…"` in the manifest
3. nothing → **local resolution**, today's behaviour, no socket opened

A resolved broker is dialed at run start — `hello` first, always — and announced with its pool
size before anything runs.

<!-- claim: unreachable-is-loud -->
A configured-but-unreachable broker is a **loud error**, never a silent fall back to local. Falling
back would turn a broken pool into a suite that quietly stopped distributing, and the only symptom
would be that it got slower.

## Frames

<!-- claim: ids-echo -->
Every request carries a client-chosen `id`; every terminal response echoes it — ids are what let
a streaming op interleave with anything else on the connection.

```json
{ "id": 1, "op": "claim", "kind": "window-server", "mode": "exclusive", "ttl_ms": 300000 }
{ "id": 1, "ok": true, "outcome": "granted", "lease": "L-7f3a", "node": "studio", "expires_at_ms": 1750000300000 }
```

<!-- claim: streams-then-terminal -->
Streaming operations emit zero or more `event` frames sharing the `id`, then exactly one terminal
frame — streamed as produced, never buffered to completion, which is what keeps a long remote run
watchable:

```json
{ "id": 4, "event": "stdout", "data": "Test Suite 'All tests' passed\n" }
{ "id": 4, "ok": true, "exit": 0 }
```

<!-- claim: unknown-op-named -->
Unknown fields are ignored, so a broker may add information without a version bump. Unknown `op`
is an error naming the op — never silence.

<!-- claim: malformed-frame-survives -->
A malformed frame is an `error`, and the connection survives it. Leases are held across turns, so
a broker that dropped the connection on a parse error would release every slot the client holds
as a side effect of a typo.

## Outcomes, and the one distinction that matters most

Every terminal frame carries `ok`, and a failed one carries `outcome`:

| `outcome` | Meaning | What prova does |
|---|---|---|
| `granted` | the request succeeded | proceed |
| `busy` | satisfiable, but not right now (+ `retry_after_ms`) | **wait and retry** |
| `unsatisfiable` | no node in the pool can ever satisfy this | **skip**, with the reason |
| `error` | the broker or the work failed (+ `message`) | **fail loudly** |

<!-- claim: busy-is-not-unsatisfiable -->
`busy` and `unsatisfiable` must never be confused, in either direction: contention is `busy`
(satisfiable, wait), absence is `unsatisfiable` (skip, carrying its reason — the only artifact a
silent skip leaves). A skip is silent by design — it is how `requires` reports "you don't have
Docker" without failing your build. Contention is not a reason to skip: reporting a saturated
pool as `unsatisfiable` converts every capacity shortage into a suite that reports green having
tested nothing — and a slot nobody offers reported as `busy` would hang a run retrying forever.
This is the single most important conformance rule in this document, and it is why quota
exhaustion (below) is `busy`.

## Operations

### `hello` — negotiate

```json
→ { "id": 0, "op": "hello", "protocol": "1.0", "client": "prova/0.14.0", "run": "R-91c2" }
← { "id": 0, "ok": true, "protocol": "1.0", "broker": "fleetd/0.1.0", "features": ["exec", "materialize"], "nodes": 4 }
```

<!-- claim: hello-negotiates -->
Version is `major.minor`. A broker MUST accept any client whose major matches and whose minor is
`<=` its own, and MUST report the version it will actually speak.

<!-- claim: features-gate-planes -->
`features` advertises optional planes, always as a list (empty is an answer; an omitted key is an
old broker that forgot to say). Prova must not send an op the broker did not advertise, and a
broker must refuse a plane it never claimed rather than half-implement it.

<!-- claim: hello-first -->
`hello` is mandatory and first. A broker MUST reject any other op on a connection that has not
said hello — a client that skips it has almost certainly failed to negotiate a version.

### `resolve` — widen `requires`

```json
→ { "id": 1, "op": "resolve", "capabilities": [{ "name": "dotnet", "constraint": ">= 9" }],
                              "toolchain": { "os": "macos", "arch": "arm64" } }
← { "id": 1, "ok": true, "outcome": "granted", "nodes": 2 }
```

<!-- claim: constraints-evaluated -->
`capabilities` mirrors `requires` exactly, including its version-constraint grammar — and a
constraint is **evaluated**, never merely refused. A broker that parsed the name and dropped the
constraint would place version-gated work on toolchains it declared unusable; one that refused
every constrained capability would make the same work skip silently forever.

<!-- claim: resolve-is-conjunctive -->
Several capabilities are one question about one node: they resolve **conjunctively**, and the
refusal names the missing one. A broker that answered disjunctively would place work on a node
missing half its requirements.

<!-- claim: resolve-counts-not-rosters -->
`nodes` is a **count, not a roster**: prova needs to know whether the work can run, never where.
Node identity is the broker's business, and keeping it out of the response keeps it out of
prova's model. An empty capability list is the common case, not an error — a proof that demands
nothing runs anywhere.

<!-- claim: resolve-advisory -->
Resolution is advisory and may be stale — a pool converges eventually. It reserves nothing: a
`resolve` that says two nodes can serve does not promise a subsequent `claim` will be granted;
that is what `busy` is for.

### `claim` / `renew` / `release` — widen `resources`

```json
→ { "id": 2, "op": "claim", "kind": "window-server", "mode": "exclusive",
              "capabilities": [{ "name": "xcodebuild" }], "ttl_ms": 300000 }
← { "id": 2, "ok": true, "outcome": "granted", "lease": "L-7f3a", "node": "studio",
              "expires_at_ms": 1750000300000 }
```

<!-- claim: modes-mirror-access-grammar -->
`kind` is the resource name from `prova.writes(…)` / `prova.reads(…)`; `mode` is `exclusive` for
a writer and `shared` for a reader — readers coexist, a writer excludes everyone, in both
directions. Prova's existing access-mode grammar *is* the slot grammar — no new vocabulary
appears at the call site.

<!-- claim: leases-expire -->
Leases **expire**. Every grant carries an identity and an `expires_at_ms`; `ttl_ms` bounds how
long a slot can be held by a client that has stopped existing: a killed `prova` must not strand a
GUI slot forever. The holder renews, and renewal moves the deadline out:

```json
→ { "id": 3, "op": "renew", "lease": "L-7f3a" }
← { "id": 3, "ok": true, "expires_at_ms": 1750000600000 }
```

<!-- claim: stale-renew-refused -->
Renewing an expired or unknown lease is `error`, not silent re-grant — the slot may already be
held by someone else, and pretending otherwise double-books it.

<!-- claim: release-idempotent -->
`release` returns the slot for the next claim. It is idempotent: releasing twice is `ok`, because
a deferred release plus an explicit one is correct teardown, not an abuse.

**Allocation is node-local.** A slot lives on exactly one machine, which is its sole writer, so
granting needs no distributed lock and no consensus. A broker's view of a peer's free capacity is
advisory; a claim that loses a race comes back `busy` and prova retries. This is the property that
lets the mesh be AP — partition-tolerant, quorum-free, severable — without ever double-granting.

<!-- claim: drain-never-preempt -->
**Drain, never preempt.** A node leaving the pool stops granting new leases and lets in-flight
ones run to completion. A broker MUST NOT revoke a granted lease. A preempted test is
indistinguishable from a failing test, so preemption would make a proof runner manufacture false
reds — the one thing it may never do. (This differs from request-scoped systems, where a retry is
transparent.)

### `exec` — run on the lease

```json
→ { "id": 4, "op": "exec", "lease": "L-7f3a", "argv": ["just", "uitest"], "cwd": "…", "env": {} }
← { "id": 4, "event": "stdout", "data": "…" }
← { "id": 4, "ok": true, "exit": 0 }
```

Execution goes *through* the broker rather than prova being handed an SSH endpoint. That keeps
credentials, host trust and transport entirely behind the socket, and it means the local reference
broker exercises the same code path as a clustered one.

<!-- claim: exec-reports-the-works-exit -->
A command that exits non-zero is a **successful exec of a failing command**: `ok` stays true and
`exit` carries the command's own code. `error` is reserved for the transport — the broker or the
spawn failing. Collapsing the two would make every red test look like a broken pool, and the fix
for those is not the same.

<!-- claim: exec-needs-a-live-lease -->
`exec` requires a granted lease. Executing against an expired or unknown lease is `error` —
running unleased work is exactly the double-booking this model exists to prevent.

### `materialize` — the workspace at a commit

```json
→ { "id": 5, "op": "materialize", "lease": "L-7f3a", "vcs": "jj",
              "change": "uwlzrpzztqwx", "source": "…" }
← { "id": 5, "ok": true, "path": "/var/…/ws-uwlzr", "warmth": { "shared_ancestor": "tvvknzpplksl" } }
```

**Place by change id, never by branch name.** A branch name is mutable and means different things
on different machines; a change id is content-addressed and means exactly one tree everywhere.
Coordinating "host and executor on the same branch" stops being a task and becomes a property.

This is where jj earns its place: the working copy *is* already a commit, so `@` has an
addressable id at every keystroke. A host can place work-in-progress without ceremony and without
rsync's failure mode of shipping dirty state that no commit describes.

`warmth.shared_ancestor` reports the nearest common ancestor between the requested change and what
the node already has materialized — the input to a scheduler's rebuild-cost estimate. A cold node
returns no ancestor.

<!-- claim: unfetchable-refused -->
Whatever a broker cannot fetch or does not speak, it **refuses by name** — an unknown change id
and an unsupported `vcs` are both `error`, never a silently different tree. Materializing
something else — trunk, an empty workspace — is the worst available outcome: the suite runs,
passes, and proves nothing about the code you meant to test.

<!-- claim: materialize-lease-bounded -->
`materialize` requires a granted lease, and the lease bounds the workspace's lifetime. Without
that, a client could fill a node's disk with trees nobody is scheduled to use, and nothing would
ever clean them up.

## Metering

A broker may be licensed; prova contains no licence logic, which is another reason the seam is a
socket rather than a linked library.

Where a broker meters concurrent leases against an entitlement, the pool is eventually consistent,
so a fleet-wide concurrent cap is **approximate by construction** — enforcing one exactly would
require the coordinator that quorum-free membership exists to avoid. The recommended shape is a
**per-node quota derived from the entitlement and gossiped**: each node self-limits, quotas sum to
the entitlement, enforcement stays a local decision. Slightly under-utilizes a skewed pool; never
stalls on a partition.

Protocol consequence: **a claim refused for quota is `busy`, never `unsatisfiable`.** The capability
exists and will be available; the client is being asked to wait. Reporting it as unsatisfiable
would silently skip tests because of a billing threshold, which is exactly the failure the outcome
distinction above exists to prevent.

## What this buys the single-machine case

The reference local broker is not a stub to be replaced — it ships a capability prova does not
have today. `materialize` against a local node is **worktree isolation**: run a suite against an
isolated tree at a change id while you keep editing. Tests stop seeing half-finished work, and it
shares its code path with the distributed case.

## The reference broker's role (`prova broker`)

Spec scaffolding, deliberately. Using prova never requires it: with no broker configured, prova
resolves everything in-process — single-machine, zero setup — and that stays the default forever.
The reference broker exists so the conformance suite can prove this document on any unix machine
(the suite spawns it per proof and throws it away), so broker implementers have a working example
to read instead of a product to reverse-engineer, and so the protocol always has a second
implementation keeping it honest. It is a **pool of one by construction** — no discovery, no
pairing, no trust, no cross-node anything; every capability it has is a strict subset of what
prova already does locally. The upgrade path is the product's, not this binary's: install a
clustered broker and name its socket, and the same suites become pool-aware — nothing about how
a proof is written changes.

## Conformance

`proofs/spec/placement/` is the executable form of this document — the suite any broker proves
itself against, including the reference one. It is hermetic: with no address named, each proof
spawns the MIT reference broker (`prova broker --socket <path> --offer <kind>`), so the spec
stays attested on any unix machine with no setup. Point the same suite at any other
implementation to conformance-test it:

```bash
PROVA_PLACEMENT_BROKER=unix:///tmp/broker.sock prova -k placement
```

The suite began as open promises and graduated with the reference broker in one proof-carrying
change — the `promises` → `proves` mechanic working as designed.
