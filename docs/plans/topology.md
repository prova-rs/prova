# Plan: topology — ephemeral resource topologies you can test *and* inhabit

Design: [`docs/design/topologies.md`](../design/topologies.md). The vision: one Lua topology,
described once, consumed by `prova test` (assert + teardown) **and** `prova up` (provision + print
endpoints + hold), so the test env and the dev env can't drift. The `scope + ctx:manage` abstraction
is already verb-agnostic — the mode just sets the held scope's lifetime.

## Done (pushed)

- **`prova.topology(name, [scope,] fn)`** — a named fixture (default `Scope.File`) also addressable
  by name. In test mode it is used exactly like a fixture (`t:use(handle)`).
- **`prova up <topology>`** (attached) — provisions the named topology under a held File scope, prints
  each resource's `url`, blocks on SIGINT/SIGTERM, then runs the normal `ctx:manage`/`teardown_scope`.
- **`prova start` / `down` / `ps`** (detached) — a thin supervisor over attached `up`. `start` spawns
  `prova up <name>` in its own process group (survives the parent, ignores parent Ctrl-C, stdio→log),
  the child self-registers a run-state record (`<home>/running/<name>.json`: pid + endpoints), `down`
  reads it and SIGTERMs the pid so the **detached child** runs the real in-process teardown, `ps`
  lists running/stale records. One provisioning path, one teardown path — no separate teardown impl.
- **`examples/topology/`** — a seeded Postgres + Redis topology, verified live across all three verbs.
- **Three port modes — external reachability done.** One definition, three behaviours, chosen by the
  verb (the definition never changes):
  1. **Testing** — random host ports (parallel-safe, collision-free). `prova` / `prova test`.
  2. **Inhabited, random** — `prova up`/`start` print each resource's endpoint, so many topologies can
     be stood up at once without port collisions.
  3. **Inhabited, fixed** — `prova up`/`start --fixed` pin each published port to its canonical
     container port (e.g. postgres `5432`, redis `6379`), so external tools connect on a predictable
     address and advertised-listener resources (Kafka) can compute their listener.

  Mechanism: `RunConfig` carries a `PortMode` (`Auto`/`Fixed`), exposed to Lua as `prova.ports`
  (`"auto"`/`"fixed"`); `prova.containerized` reads it and upgrades random ports to fixed bindings
  under `--fixed` (author-fixed `{ container, host }` entries are left untouched). Verified live: `up
  --fixed orders` binds and is reachable on `5432`/`6379`; default `up` uses random ports. (`--fixed`
  on `prova start` forwards to the detached `prova up`.)

- **`prova watch <name>`** — done. The inhabited dev loop: provision → print endpoints → hold; on any
  change to the definition files, tear down and re-provision from the *fresh* definition (new Lua
  state) and re-print. Dependency-free mtime polling (400 ms) with a 250 ms settle so one save →
  one re-apply. A definition that fails to provision (a bad edit) is reported and the loop keeps
  waiting for the fix rather than exiting. Attached-only; pair with `--fixed` so endpoints stay put
  across re-applies. Verified live against `examples/topology` (touch → single clean re-apply → Ctrl-C
  teardown, no orphaned containers). `up`/`watch` now share `build_topology_run` (CLI) and
  `load_topology`/`provision` (engine).

## Remaining

_None blocking — the topology track is resolved. Two items are intentionally left as
future/plugin-side work:_

1. **Topology addressing (per-resource)** — addressing a *whole* topology by name across all verbs is
   done (`up`/`watch`/`start`/`down`/`ps` all take a name; several coexist under random ports). The
   *reachability* half is settled by the port modes. What remains — standing up or referencing an
   *individual* resource (`prova up orders.db`) — is speculative and likely a non-goal; deferred until
   a real need appears.
2. **Advertised-listener recipe (Kafka)** — the core signal (`prova.ports` + fixed bindings) is in
   place; authoring a Kafka topology that reads it is a plugin-side follow-up, not core work. Left for
   when a real Kafka topology is wanted.

## Notes

- Testing stays the wedge (narrow go-to-market); inhabiting is the same engine, one layer up.
- Positioning line: *"Prova — acceptance testing where real resources are first-class."*
