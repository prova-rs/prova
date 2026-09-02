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
to it contend unpredictably unless the suite says the rule out loud. One writer token —
`locks = { prova.writes("cargo") }` — named once on every cargo-invoking proof, is the whole fix.

## Scope

- **Package** (the default): every prova instance at THIS home shares the namespace. Two
  unrelated repos never collide on a token name.
- **Machine**: `prova.writes("gpu", { scope = "machine" })` — the rule spans every repo on the
  box. `prova.port(N)` is machine-scoped by default, because a host port is machine-wide fact.

## The lock file is the contract — joining from outside

A package lock is a `flock` on `<home>/.prova/var/locks/<token>.lock` — prova's process is not
the boundary. The `[runner]` provision holds its manifest-declared tokens (`locks = ["cargo"]`)
while it builds, and any external tool joins the same rule by holding the same file:

```sh
# Any shell, any OS — the contract's portable spelling (a bare token is a WRITE
# hold; --reads is the concurrent one; --machine spans repos):
prova lock cargo -- cargo build
prova lock db --reads -- ./report.sh
```

From Rust, `prova_core::locks::hold_exclusive("cargo", Some(&home))?` blocks and releases on
drop. Any language with flock bindings joins directly (~5 lines: open the path, `LOCK_EX` for a
writer, `LOCK_SH` for a reader); Linux shell also has `flock(1)`. The kernel releases a crashed
holder instantly.

## Waiting is visible, and who you wait on is nameable

A queued hold says so, with how long it waited — the leaf, the `[runner]` provision, and the
wrapper (`waited 651.2s for lock "cargo", ran 190.3s`). Short waits stay quiet. Every run banks
`run.lock_wait_ms` — wall time **stalled** on another instance's lock — in the `timings` baseline.

A `flock` tells the kernel everything and you nothing, so a hold also writes a record of itself
beside the lock file. `prova locks` reads both — held-ness from the kernel, identity from the
records:

```
$ prova locks --machine
machine  (/tmp/prova-locks)
  HELD  cargo
          writer pid 49213 — prova lock cargo -- cargo test (in /repo)
  free  port-5432
```

A blocking wait says the same while it waits: the holder by name on the first line, then again
every 60s (`PROVA_LOCK_NARRATE_EVERY`). Bound it with `prova lock --wait-timeout 30m` or
`PROVA_LOCK_WAIT_TIMEOUT`; **unbounded is the default**, because failing a queue is the wrong cure
for a slow holder. **`HELD` with nobody named is normal** — a Makefile or `flock(1)` owes prova no
record — and reads as `unregistered holder`, never as free. Nothing can *release* another
process's flock, only end that process, so naming the holder is the recourse on offer.

## Vocabulary, precisely

- `locks` — this page: tokens the scheduler holds. (`resources = { … }` is the deprecated
  spelling; it warns and retires at 1.0. A topology **resource** — postgres, kafka — is a
  different thing: a provisioned service. `prova learn topologies`.)
- `serial = true` — **run-scoped** whole-run exclusivity: this run's own parallelism dial. A
  rule that must bind other instances is spelled as a lock.
- Re-moding composes: `prova.reads(prova.port(5432))` widens a port to a concurrent hold.

See also: `prova learn running` (--jobs is throughput only) · `prova learn verifiers` (the classic
exclusive resource) · `prova learn fixtures` (where shared tools live)
