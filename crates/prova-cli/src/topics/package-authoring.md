# package-authoring — package a capability others require()

Scaffold: `prova init package` (see `prova learn init`). ONE archetype, two shapes, decided by
WHERE you run it: inside a package it scaffolds a LOCAL package into the `packages` directory
(core files only, `require()`-able with zero declaration); outside — or with `-s standalone`
anywhere — a repo-ready STANDALONE package (core + LICENSE/CI/.version-line) consumers pin as a
git dep. Same core either way, so graduating local → standalone is a directory move. Every
package is a prova package — its manifest is a `prova.toml` with a `[package]` section, and it
self-tests with its own proofs (a LOCAL package's manifest makes it a nested-package boundary,
so its proofs stay out of the owning project's suite — run them from inside its directory).

```
my-package/
  prova.toml          # dual-role: run manifest + package declaration
  init.lua            # the namespace table (entry point)
  library/<name>.lua  # LuaCATS stub — REQUIRED for introspection + IDE to see you
  proofs/             # the package's own self-test
```

```toml
[package]
name        = "greet"
entry       = "init.lua"          # precedence: consumer module= > entry > init.lua > <alias>.lua
description = "..."
[[package.topologies]]            # optional: advertise topologies by name
name = "vm"
factory = "topologies.vm"
requires = ["prlctl"]
[requires]
prova = ">=0.4"                   # compat-gated against the running prova
```

## The contract

- Return ONE namespace table. Context-first calls: `greet.thing(ctx, opts)`.
- Resource packages follow the facet grammar — `client` / `container` / `wait_for` / `mock` —
  so consumers already know your shape. `prova package lint <file>` checks it (Resource vs
  Library classification, malformed facets, missing stub).
- Lifecycle through the context: `ctx:manage(handle)` for anything with `:stop()`/`:close()`;
  never leak a process/container past the scope.
- The common body is one call: `prova.containerized{ name, image|build, port, env, url, client,
  wait }` — provision, wait, manage, and return the `{ client, url, container, host, port }`
  trio.
- A verb that changes provisioned state returns only when the change is observable THROUGH THE
  SEAM THE CONSUMER WILL USE — the provisioner's exit code is not that seam. `prova.retry` the
  consumer-visible check; a CLI that "completed" may have handed off to a store that syncs later.
- Private dependencies: a `[dependencies]` table in the PACKAGE'S own prova.toml (e.g.
  `inner = { path = "deps/inner" }`) — isolated from consumers, no version bleed.
- Ship `library/<name>.lua` (`---@meta <name>`) — it is what makes your API answerable in
  editors; lint warns without it.

## Where packages live, nearest first

| Stage | Where |
|---|---|
| Package-local (this repo only) | a dir under `[run] packages` — requirable by name, zero declaration; `prova init package` in-package scaffolds one |
| Shared, pinned | its own repo (`prova init package -s standalone`); consumers declare `[dependencies] name = "owner/repo@ref"` |
| A local file while incubating | `[dependencies] name = "./packages/name.lua"` or `-P name=./...` |

Self-test it like any package: `prova` inside the package repo runs its proofs against its own
namespace. Consumers' `[requires] prova` gate protects them from your future breakage.

Go deeper: `prova learn packages` (the consumer side) · `prova learn doubles` (the mock facet).
