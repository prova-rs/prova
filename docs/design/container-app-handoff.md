# Hand-off: `container_app` (Proof 4 — the SUT-in-a-container payoff)

Written 2026-07-16 to hand a fresh session the state needed to finish the networked-topology arc.
Read alongside [topologies.md](topologies.md) and [mcp-mode.md](mcp-mode.md).

## Where things stand

`prova-rs/prova` **main** = `82a5cbce` (all pushed, released chain at v0.2.6; unreleased work sits on
main). The networked-topology arc is **3 of 4 proofs done**, all proof-first and CI-guarded:

- **Proof 1 — primitive** (`testdata/docker_network.lua`, `tests/docker_network.rs`):
  `docker.network()` → managed bridge network; `docker.run{ network, alias }` joins a container
  dual-homed (host publish AND on the network); `container:network_alias()`. bollard
  `NetworkingConfig` endpoint aliases at create; LIFO-safe teardown.
- **Proof 2 — resource vantage** (`testdata/containerized_network.lua`,
  `tests/containerized_network.rs`): a `prova.containerized` resource given `opts.network`+`opts.alias`
  gains `resource.network = { url, host, port, alias }` (alias + CONTAINER port for in-network
  consumers; distinct from the host vantage's mapped port). The network `url` is the host `url` with
  its `127.0.0.1:<mapped>` authority token-swapped to `<alias>:<container>` — **zero plugin changes**.
- **Proof 3 — topology convenience** (`testdata/topology_network.lua`, `tests/topology_network.rs`):
  inside a `prova.topology` factory `ctx.network` is a lazily-minted scope-managed network; resources
  auto-join aliased by recipe name. **Hard invariant (holds under test): `ctx.network` is non-nil
  ONLY in a topology factory** — plain fixtures/test bodies stay un-networked, existing suites
  byte-identical. Marked at the single `resolve_use` seam (`engine.rs`), so test/`up`/warm-MCP all
  inherit it; network reaps after its containers.

## Proof 4 — what to build

`container_app`: the system under test **built and run in a container**, wired to topology resources
over the network — so a machine needs **nothing but Docker** (no host SDK/JVM/uv), and the archetype
suites shed `requires = { "dotnet" }` etc. The SUT becomes just another resource in the grammar.

**The design (from mcp-mode/topologies + the decided naming):**
- A `container_app(ctx, opts)` helper (or grow `prova.containerized` / a sibling) that:
  1. **Builds an image** — `docker build` of a Dockerfile. The archetypes already ship
     `.platform/docker/local/Dockerfile`, so the build recipe IS the project's own Dockerfile →
     you test the real production artifact, not a host-built approximation.
  2. **Runs it on the topology network** (`ctx.network`), wired from resources' **network vantage**
     (`env = { DATABASE_URL = pg.network.url }` — alias:container_port, reachable in-network).
  3. **Host-publishes** its own port so the test runner (always on the host) probes it
     (`http.wait_for(app.url .. "/health")`, `app.client:post(...)`).
  4. **Returns the standard grammar shape** `{ url (host), network, container, host, port }` — the
     SUT is dual-homed exactly like a resource.
- The author picks per fixture: host-run SUT (today's `shell.spawn`, uses resource host `url`s) or
  containerized SUT (`container_app`, uses resource `network.url`s). Both coexist — flexibility
  preserved (the two-layer invariant: conveniences never remove primitives).

**Subtasks / known hard cases:**
- **Build cache** — naive `docker build` is glacial. Mount toolchain caches (cargo registry,
  `~/.nuget`, pnpm store, uv cache) as build/volume mounts so rebuilds reuse. Warm topologies
  compound this: build on `up`, warm `run { topology }` re-runs in ms.
- **Kafka advertised-listener** — must advertise the network alias to in-network clients but the host
  address to host clients → a network-aware dual-listener mode in the kafka plugin (`INTERNAL://kafka:9092`,
  `EXTERNAL://127.0.0.1:<host_port>`). The one place dual-homing isn't free.
- **Acceptance proof (the real bar):** convert ONE archetype — `dotnet-rest-service-archetype` — to a
  containerized-SUT variant: render → `docker build` its Dockerfile → run on the topology network
  wired to `postgres.container`'s `network.url` → drive CRUD over the host-published port → cross-check
  the DB. It should drop `requires = { "dotnet" }` and need only Docker. That variant, green in CI, is
  Proof 4 done.

## The proof-first workflow (how every increment above was built)

1. Write the proof FIRST as `testdata/<name>.lua` — assert the real behavior (for networking, the true
   bar is a **sibling container reaching a resource by DNS**, not just field presence).
2. Run it, confirm RED for the right reason (`./target/debug/prova crates/prova-core/testdata/<name>.lua`).
   Confirming red repeatedly corrected the design (split primitive vs resource level; caught a
   wrong-reason red from a test-recipe client lacking `close()`).
3. Implement (self or delegate an agent with the red proof as the exact contract).
4. Green → wire into CI (`tests/<name>.rs`, mirror `tests/docker_network.rs`), update `library/*.lua`
   stubs, `jj commit` + push, `cargo install --path crates/prova-cli --force` to refresh the MCP binary.

## Gotchas that will bite (all real, hit this session)

- **Two binaries.** `cargo build -p prova-cli` → `target/debug/prova`; `cargo install --path` →
  `~/.cargo/bin/prova` (what the MCP launches). Iterate engine work on `target/debug/prova`; a stale
  `~/.cargo/bin` gave a **false red** once.
- **MCP pins its binary at launch.** The running `prova mcp` server holds whatever binary it started
  with — a rebuild is invisible until the server restarts (a new Claude Code session). So the MCP
  shines for iterating **suites against a stable prova**, NOT prova's own engine; use the CLI for engine
  work. (Prova is registered as a user-scoped MCP in BOTH `~/.claude-work` and `~/.claude-personal`,
  pointing at `~/.cargo/bin/prova`.)
- **Recurring docker flake:** docker-heavy test binaries flake under parallel `cargo test` load
  (container-start timeouts) — seen 3+ times (pulsar, docker×2). Pass solo/on rerun; NOT logic
  regressions. Fix candidate before CI leans on these: serialize the docker/network/kafka/pulsar test
  binaries behind a shared docker semaphore.
- **jj `main` can drift** when the release chore runs `jj new`/commits in *another* repo (run-action):
  the prova working copy gets stranded on a stale change while `main` is correct — `jj new 'main@origin'`
  to re-sync the checkout before working. (A stale checkout is what made a docs agent correctly REFUSE
  to write "shipped" docs — the ground truth was wrong, not the agent.)
- **gh has two keyring accounts** (`jimmiebfulton` personal ↔ `jimmiefulton-ybor` work); prova-rs/archetect
  workflow dispatches need the **personal** one: `gh auth switch -u jimmiebfulton` → dispatch →
  `gh auth switch -u jimmiefulton-ybor`.
- **mcp-loader stringifies tool `arguments`** — a harness bug (not prova's); to drive `prova mcp`
  parameterized calls outside a real MCP client, use direct JSONL batches. Native `mcp__prova__*` tools
  (the registered server) do NOT have this bug.

## Quick orientation commands

```
cd /Users/jimmie/personal/prova-rs/prova
jj log -r main -n1                                   # should be at/after 82a5cbce
cargo test -p prova-core --test docker_network --test containerized_network --test topology_network
prova eval 'return type(docker.network)'             # "function" (network primitive present)
prova skill                                          # the agent skill — full Prova idiom
```
