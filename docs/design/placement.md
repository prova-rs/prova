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

Newline-delimited JSON over a Unix domain socket — `socket.connect(ctx, { addr = addr, framing = { delimiter = "\n" } })`
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

### Queued claims — the `queue` plane

A client that would rather WAIT for a slot than retry can take a place in its queue. Polling a
busy claim blocks the waiter for the whole wait, and nothing about it is fair.

<!-- claim: queue-is-a-plane -->
Queueing is an optional plane: a broker that serves it advertises `queue` in `features`, and a
client sends `queue: true` only to such a broker. It is a FEATURE and not a protocol minor. A newer
minor is refused outright, so bumping one would cut every upgraded client off from every broker
not yet upgraded, and a pool cannot upgrade all its machines in one instant. A broker without the
plane ignores `queue` and answers `busy`, which the client handles as it always has.

```json
→ { "id": 4, "op": "claim", "kind": "window-server", "mode": "exclusive", "ttl_ms": 300000, "queue": true }
← { "id": 4, "ok": false, "outcome": "queued", "ticket": "T-3", "position": 1 }
```

<!-- claim: queued-is-fifo -->
A queued claim on a busy slot answers `queued` with a `ticket` and a 1-based `position`. The queue
is FIFO per slot, and a claim WITHOUT `queue` never jumps a non-empty queue: it answers `busy`.
Otherwise a stream of fast claimants, such as readers that could share a reader's instance, starves
a queued writer forever.

<!-- claim: handed-not-hinted -->
When the slot frees, the broker HANDS it to the head of the queue. The grant happens in the broker,
never in a message about it: the waiter learns it by refreshing its ticket. Every such answer can be
asked again. A handed ticket answers `granted` with its lease for as long as that lease lives, so a
refresh whose answer was lost is simply repeated, and it answers `error` once the lease has ended.

```json
→ { "id": 5, "op": "ticket", "ticket": "T-3" }
← { "id": 5, "ok": true, "outcome": "granted", "lease": "L-9", "expires_at_ms": 1750000900000 }
```

<!-- backlog: ticket-is-heartbeat -->
A ticket refresh is the waiter's heartbeat. A ticket not refreshed within the broker's ticket TTL
is dropped and the next waiter is served, so a waiter that died costs its successor one TTL, never
the slot. (Backlog, not yet a claim: a conformance proof needs the ticket TTL configurable on the
broker under proof. Fleet's table tests prove its implementation.)

<!-- claim: cancel-declines -->
`cancel` leaves the queue. Cancelling a HANDED ticket declines the grant: its lease is released and
the slot goes to the next waiter. It is idempotent, like `release`.

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

<!-- claim: broker-leaves-its-workspace-root recorded=2026-09-14 -->
**`materialize` creates `temp_dir()/prova-broker-<pid>/` to hold its workspaces, and nothing ever
removes it.** `drop_lease` forgets and deletes each `ws-<change>` it bounded
(`crates/prova-cli/src/broker.rs`), but the per-process root outlives the broker. Found 2026-09-14 on
a developer Mac: 220 `prova-broker-<pid>` directories in the macOS temp dir, created
2026-08-20..09-09, together under 10 MB, every owning pid gone. Empty roots cost nothing; they are
the visible half of a gap in the claim above — the lease bounds a workspace only while the broker
lives to drop it. A broker that is killed or crashes with leases outstanding strands the `ws-*` trees
and their `jj workspace` registrations in the source repo, and nothing is scheduled to find them.
Proposed: remove the root when its last lease drops and at shutdown; and at broker start, sweep
`prova-broker-<pid>` roots whose pid is dead (forgetting their `prova-broker-<pid>-*` workspaces
where the source repo is known).

**Resolved.** The root is `<base>/broker-<pid>/` (`scratch::owned_root`), the same owner-tagged
convention run scratch uses — so it is reaped by the startup sweep in **any** later prova, not just
a later broker, which is the difference between self-healing and hoping the right program runs
next. It is also removed when the last workspace drops, with `remove_dir` rather than
`remove_dir_all`: that succeeds only on a genuinely empty directory, so a workspace this bookkeeping
has lost track of is left for the sweep to find instead of being deleted by a cleanup that had
already stopped knowing what was in there. Forgetting a dead broker's stranded `jj workspace`
registrations stays open — the sweep removes the trees, but deregistering needs the source repo
path, which the root's name does not carry.

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
