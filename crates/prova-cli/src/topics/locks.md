# locks — house rules the scheduler holds, across every prova instance

A **lock** is a named token a test holds while it runs, with readers-writer semantics:
`prova.writes(token)` is an exclusive hold, `prova.reads(token)` a concurrent one,
`prova.port(N)` an exclusive hold on a host port. Declare them on the unit (or a group — they
flow down):

```lua
prova.test("builds the workspace", {
  locks = { prova.writes("cargo") },
}, function(t)
  t:expect(shell.run({ "cargo", "build" }).code):equals(0)
end)
```

The scheduler co-schedules everything whose locks are compatible and queues the rest — inert at
`--jobs 1`, enforced above it, **and held across prova instances**: every lock is also a file
lock (`flock`) under the package's `var/`, so `-j 10`, a second agent's run, and CI on the same
box all obey one rule. A leaf blocked by another instance's hold simply waits its turn. The
kernel releases a crashed holder's locks instantly — no daemon, no stale-lock cleanup.

## The canonical house rule

Cargo (and most build tools) takes process-wide locks of its own; two suites that both shell out
to it contend unpredictably unless the suite says the rule out loud. One writer token, named once
on every cargo-invoking proof, is the whole fix:

```lua
locks = { prova.writes("cargo") }
```

## Scope

- **Package** (the default): every prova instance at THIS home shares the namespace. Two
  unrelated repos never collide on a token name.
- **Machine**: `prova.writes("gpu", { scope = "machine" })` — the rule spans every repo on the
  box. `prova.port(N)` is machine-scoped by default, because a host port is machine-wide fact.

## The lock file is the contract — joining from outside

A package lock is a `flock` on `<home>/.prova/var/locks/<token>.lock` — prova's process is not
the boundary. The `[runner]` provision holds its manifest-declared tokens (`locks = ["cargo"]`)
while it builds, and any external tool joins the same rule by holding the same file:

```rust
// Rust (what this repo's xtask does):
let _hold = prova_core::locks::hold_exclusive("cargo", Some(&home))?;  // blocks; drop releases
```

```sh
# Any shell, any OS — the contract's portable spelling (a bare token is a WRITE
# hold; --reads is the concurrent one; --machine spans repos):
prova lock cargo -- cargo build
prova lock db --reads -- ./report.sh
```

Any language with flock bindings joins directly (~5 lines: open the path, `LOCK_EX` for a
writer, `LOCK_SH` for a reader); Linux shell also has `flock(1)`. The kernel releases a
crashed holder instantly.

## Waiting is visible, not inferred

A queued hold says so, with how long it waited — the leaf (`waiting for lock(s) cargo (held by
another prova instance) — my test is queued`), the `[runner]` provision, and the wrapper
(`waited 651.2s for lock "cargo", ran 190.3s`). Short waits stay quiet, so ordinary
serialization does not chatter. Every run also banks `run.lock_wait_ms` — the wall time it
spent **stalled** on a lock another instance held — in the `timings` baseline set, so a reminder
can watch contention become the bottleneck instead of an operator diffing sibling logs.

## Vocabulary, precisely

- `locks` — this page: tokens the scheduler holds. (`resources = { … }` is the deprecated
  spelling; it warns and retires at 1.0. A topology **resource** — postgres, kafka — is a
  different thing: a provisioned service. `prova learn topologies`.)
- `serial = true` — **run-scoped** whole-run exclusivity: this run's own parallelism dial. A
  rule that must bind other instances is spelled as a lock.
- Re-moding composes: `prova.reads(prova.port(5432))` widens a port to a concurrent hold.

See also: `prova learn running` (--jobs is throughput only) · `prova learn verifiers` (the classic
exclusive resource) · `prova learn fixtures` (where shared tools live)
