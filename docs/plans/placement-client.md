---
title: The placement client — staging, and the dispatch decision
type: plan
maturity: in-flight
tags: [plan, placement, client, broker, dispatch]
---

# The placement client — where the wiring stands, and the decision it stops at

Written 2026-08-04, the night the reference broker landed and the conformance suite graduated.
The transport foothold shipped (see below); this memo records why the *semantic* wiring —
`requires`/`resources` actually answered by the broker — deliberately did not, and what has to be
decided before it can.

## Shipped: the transport foothold

`[placement] broker` in the manifest, `PROVA_PLACEMENT_BROKER` over it, blank-means-unset. A
configured run dials the broker at start, `hello` first, announces the pool, and proceeds;
configured-but-unreachable is a loud exit 2 before any proof loads, naming the address and where
it was configured. `local is never a fallback` is pinned black-box (the child's marker file stays
absent). Anchored: placement.md `#broker-address-resolution`, `#unreachable-is-loud`, both
attested by `proofs/spec/placement/transport_test.lua`.

What this buys tonight: the configuration surface exists and is validated loudly, the protocol
handshake runs in the real run path, and pointing a machine's manifest at fleetd already
*works* — prova negotiates, announces the pool, and runs. It just doesn't yet *ask the pool
anything*.

## Not shipped, on purpose: routing `requires` / `resources` through the broker

The blocker is not effort — it is that **pool answers are only sound once work can be placed on
the node that answered.** Today every leaf executes locally. Wire `requires` to `resolve` now and
a `requires = { "docker" }` proof runs *locally, on a docker-less laptop*, because a peer across
the room has docker — `resolve` said `granted`, the leaf ran here, and it fails on a machine it
was never meant for. That manufactures false reds, the one thing a proof runner may never do.
`resources` has the same defect through the other door: a lease granted on `node = "studio"` is
meaningless while the work runs on `node = "laptop"` — worse, it *excludes* the machine actually
entitled to run.

`resolve` is a node **selector** (the doc says exactly this). A selector without a dispatcher is
a coin flip wearing a seatbelt.

## The decision for the morning: what "dispatch" means for prova

Three shapes, not mutually exclusive, roughly in ascending ambition:

1. **Leaf-granular remote exec.** The scheduler claims a slot, materializes the workspace at the
   run's change id, and runs *the leaf* on the granted node via `exec` (`prova --node <path>`
   remotely), replaying the JSONL event stream into the local reporters. Maximum granularity,
   but every leaf pays materialization + process startup, and cross-leaf state (file-scope
   fixtures) fragments.
2. **Suite-granular placement.** The unit of dispatch is the suite (already prova's scheduling
   unit — `jobs` counts suites). A suite whose `requires` only resolve remotely is shipped as one
   `exec` of `prova <suite>` on the node, others run locally. Fixtures stay whole; the event
   stream replays per suite. This is the shape the run-record and reporter machinery already fit.
3. **Whole-run offload.** `prova --on <capability-expr>` runs the entire suite remotely —
   the simplest thing that makes "test on the mac studio from the laptop" real, and a decent v1
   even though it is closer to a remote shell than to placement.

Recommendation: **2**, with 3 as the possibly-same-week warm-up. The suite is already the
scheduling unit, the reporter fan-out already merges multiple suites' events, and per-suite
`requires` union is already computed for skip resolution. Leaf-granular (1) buys granularity
nothing in prova's model currently rewards.

Also decided-by-argument but worth Jimmie's eyes:

- **The mixed-run rule.** When a broker is configured, a suite's `requires` should be answered
  by: local capabilities first (project-registered vocabulary like `soak` stays local — a broker
  cannot know a project's `prova.lua`), then the broker for what remains unmet locally — and a
  broker-satisfiable-only suite *must be dispatched, not run locally*. Absent dispatch (today),
  the only honest answers are all-local, which is exactly current behavior.
- **`resources` with a broker present** should claim through the broker *for dispatched work
  only*; local work keeps the in-process semaphore. One asterisk: two *local* prova processes
  could use the local broker as a cross-process lock — real value, but it changes lease kinds
  from "pool slots" to "anything anyone names", which the reference broker's `--offer` model
  deliberately refuses. If cross-process locking is wanted, it should be its own decision, not
  a side effect.
- **Renewal cadence**: a dispatched suite's lease renews from the client on a timer derived from
  `ttl_ms` (half-life), and a failed renewal *fails the suite loudly* — never silently re-runs.

## The proof shape for the dispatch phase (written before the code, when it starts)

- A fake broker fixture (Lua, `socket.listen`) that scripts `busy → granted`, records claim
  order, and asserts renewal cadence — the client-behavior mirror of the broker conformance
  suite.
- End-to-end against the reference broker: a suite `requires` something the local machine lacks
  but the broker offers via a scripted capability — dispatched, events replayed, record
  attributes the node.
- The drain case from the client side: a lease that expires mid-suite fails that suite loudly
  and releases nothing else.

## Sequencing

1. This memo reviewed; dispatch shape picked (recommendation: suite-granular).
2. Client library grows claim/renew/release + exec/materialize planes (the broker side of all of
   these is already conformance-tested; the client side gets the fake-broker suite).
3. Scheduler integration behind the same loud-or-nothing rule as the transport.
4. fleetd closes its remaining conformance gaps (constraint evaluation, forwarded lease
   lifecycle, materialize) — then the same dispatch works cross-machine with zero prova changes.
