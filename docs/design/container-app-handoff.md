# Hand-off: the containerized SUT — mechanism landed, archetype bar open

Written 2026-07-16 for the `container_app` build; **updated 2026-07-16** now that the mechanism is in.
Read alongside [topologies.md](topologies.md) (which records the design outcome) and
[mcp-mode.md](mcp-mode.md).

## Where things stand

The networked-topology arc is **4 of 4 proofs done**, all proof-first and CI-guarded:

- **Proof 1 — primitive** (`testdata/docker_network.lua`, `tests/docker_network.rs`):
  `docker.network()` → managed bridge network; `docker.run{ network, alias }` joins a container
  dual-homed; `container:network_alias()`.
- **Proof 2 — resource vantage** (`testdata/containerized_network.lua`, `tests/containerized_network.rs`):
  a resource on a network gains `resource.network = { url, host, port, alias }` — alias + CONTAINER
  port for in-network consumers. Zero plugin changes.
- **Proof 3 — topology convenience** (`testdata/topology_network.lua`, `tests/topology_network.rs`):
  `ctx.network` is a lazily-minted scope-managed network; resources auto-join aliased by recipe name.
  **Hard invariant: `ctx.network` is non-nil ONLY in a topology factory.**
- **Proof 4 — the containerized SUT** (`testdata/docker_build.lua` + `tests/docker_build.rs`;
  `testdata/container_app.lua` + `tests/container_app.rs`): **`docker.build{}`** (the primitive) plus
  **`prova.containerized{ build = … }`** (the SUT as a resource — `build` where a published resource
  writes `image`). Proved end-to-end: a service built from a nested Dockerfile, on the topology
  network, reaching postgres by DNS alias, driven black-box over HTTP from the host. See
  [topologies.md](topologies.md#the-containerized-sut--build-instead-of-image-done) for the design and
  the reasoning behind the CLI shell-out (BuildKit cache mounts + `.dockerignore`).

**The naming decision the earlier hand-off left open is settled:** there is no `container_app` helper.
A SUT is not a second concept — it is a `prova.containerized` resource whose image is built. That
inherits auto-join / vantage / readiness / teardown for free and cost ~15 lines.

## What's next — the archetype acceptance bar (the real proof)

The mechanism is proved against a purpose-built service. The bar that matters is a **real** one:
convert `dotnet-rest-service-archetype` to a containerized-SUT variant — render → `docker build` its
own `.platform/docker/local/Dockerfile` → run on the topology network wired to `postgres.container`'s
`network.url` → drive CRUD over the host-published port → cross-check the DB. It should **drop
`requires = { "dotnet" }` for `requires = { "docker" }`**. That variant, green in CI, is the arc
actually paying off. It lives in the archetype suites (p6m-archetypes3), not here.

Expect the **build cache** to be the work: this repo's proof builds a trivial image, so it has not
exercised a real toolchain. Mount the toolchain caches (`~/.nuget`, cargo registry, pnpm store, uv) as
BuildKit cache mounts in the Dockerfile — `docker.build` already routes through the CLI specifically
so `RUN --mount=type=cache,…` works. Warm topologies compound it: build on `up`, re-run in ms.

Also still open (both named in [topologies.md](topologies.md#remaining-work-bounded-and-named)):
**Kafka's advertised listener** (the one resource dual-homing isn't free for), and **`wait = { port }`
being a coarse signal** (below).

## The proof-first workflow (how every increment above was built)

1. Write the proof FIRST as `testdata/<name>.lua` — assert the real behavior (for networking, the true
   bar is a **sibling container reaching a resource by DNS**, not field presence).
2. Run it, confirm RED **for the right reason** (`./target/debug/prova crates/prova-core/testdata/<name>.lua`).
3. Implement (self, or delegate an agent with the red proof as the exact contract).
4. Green → **verify the green is real** (see mutation-testing below) → wire into CI (`tests/<name>.rs`),
   update `library/*.lua` stubs, `jj commit` + push, `cargo install --path crates/prova-cli --force`.

## Gotchas that will bite (all real, all hit in anger)

- **A green deserves the same scrutiny as a red.** Proof 4 was mutation-checked by swapping
  `db.network.url` → `db.url` and confirming it goes red; without that, "passes" doesn't prove it
  tests what it claims. Cheap (`sed` into a scratch file), and it caught nothing here only because the
  proof was right — it is still the step that earns the confidence.
- **`wait = { port }` is a FALSE-READY on Docker Desktop.** It probes the *mapped host* port, and the
  port proxy accepts before the server inside is listening — measured: the first `psql` after "ready"
  fails. Recipes survive only because `prova.containerized` wraps client factories in `prova.retry`; a
  black-box resource with no client factory has no backstop. Don't trust `wait` as a true gate.
- **Green-by-luck is real, and timing changes expose it.** Making `docker.run` skip a redundant pull
  removed ~500ms that had been silently carrying Proof 1's boot race. A performance change turning a
  test red is evidence about the *test*, not just the change — Proof 1 had been passing on latency.
- **`docker.run` pulls only if the image isn't local** (as of Proof 4) — `docker run`'s own rule.
  Before that, an unconditional pull made every locally-built image fail with a misleading "pull
  access denied / repository does not exist".
- **Image tags must be STABLE, unlike network names.** Networks are cheap and must not collide, so
  they're unique-per-run. Images are expensive and want reuse: `docker.build`'s default tag is derived
  from the context path, so a rebuild *replaces* it and hits the layer cache. A unique-per-run image
  tag would leak a dangling image every run.
- **Two binaries.** `cargo build -p prova-cli` → `target/debug/prova`; `cargo install --path` →
  `~/.cargo/bin/prova` (what the MCP launches). Iterate engine work on `target/debug/prova`; a stale
  `~/.cargo/bin` gave a **false red** once.
- **MCP pins its binary at launch.** A rebuild is invisible until the server restarts (a new session).
  The MCP shines for iterating **suites against a stable prova**, NOT prova's own engine.
- **Recurring docker flake:** docker-heavy test binaries flake under parallel `cargo test` load
  (container-start timeouts) — seen 3+ times. Distinguish it from a real failure by **speed**: the
  flake is a timeout (slow); a logic failure fails fast. Fix candidate: serialize the
  docker/network/kafka/pulsar binaries behind a shared docker semaphore.
- **jj `main` can drift** when a release chore runs `jj new`/commits in *another* repo (run-action):
  `jj new 'main@origin'` to re-sync the checkout before working.
- **gh has two keyring accounts** (`jimmiebfulton` personal ↔ `jimmiefulton-ybor` work); prova-rs
  dispatches need the **personal** one: `gh auth switch -u jimmiebfulton` → dispatch → switch back.
- **mcp-loader stringifies tool `arguments`** — a harness bug; use direct JSONL batches to drive
  `prova mcp` outside a real MCP client. Native `mcp__prova__*` tools do NOT have this bug.

## Quick orientation commands

```
cd /Users/jimmie/personal/prova-rs/prova
cargo test -p prova-core --test docker_network --test containerized_network \
                         --test topology_network --test docker_build --test container_app
prova eval 'return type(docker.build)'               # "function" (the build primitive)
prova skill                                          # the agent skill — full Prova idiom
```
