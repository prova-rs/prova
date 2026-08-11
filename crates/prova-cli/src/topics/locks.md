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

## Vocabulary, precisely

- `locks` — this page: tokens the scheduler holds. (`resources = { … }` is the deprecated
  spelling; it warns and retires at 1.0. A topology **resource** — postgres, kafka — is a
  different thing: a provisioned service. `prova learn topologies`.)
- `serial = true` — **run-scoped** whole-run exclusivity: this run's own parallelism dial. A
  rule that must bind other instances is spelled as a lock.
- Re-moding composes: `prova.reads(prova.port(5432))` widens a port to a concurrent hold.

Go deeper: `prova learn authoring` (the opt grammar) · `prova learn topologies` (provisioned
resources) · `prova learn running` (`--jobs`).
