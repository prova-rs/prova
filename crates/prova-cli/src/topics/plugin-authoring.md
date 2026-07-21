# plugin-authoring — package a capability others require()

Scaffold: `prova init plugin` (see `prova learn init`). A plugin is itself a prova package —
its manifest is a `prova.toml` with a `[plugin]` section, and it self-tests with its own proofs.

```
my-plugin/
  prova.toml          # dual-role: package manifest + plugin advertisement
  init.lua            # the namespace table (entry point)
  library/<name>.lua  # LuaCATS stub — REQUIRED for introspection + IDE to see you
  proofs/             # the plugin's own self-test
```

```toml
[plugin]
name        = "greet"
entry       = "init.lua"          # precedence: consumer module= > entry > init.lua > <alias>.lua
description = "..."
[[plugin.topologies]]             # optional: advertise topologies by name
name = "vm"
factory = "topologies.vm"
requires = ["prlctl"]
[requires]
prova = ">=0.4"                   # compat-gated against the running prova
```

## The contract

- Return ONE namespace table. Context-first calls: `greet.thing(ctx, opts)`.
- Resource plugins follow the facet grammar — `client` / `container` / `wait_for` / `mock` —
  so consumers already know your shape. `prova plugin lint <file>` checks it (Resource vs
  Library classification, malformed facets, missing stub).
- Lifecycle through the context: `ctx:manage(handle)` for anything with `:stop()`/`:close()`;
  never leak a process/container past the scope.
- The common body is one call: `prova.containerized{ name, image|build, port, env, url, client,
  wait }` — provision, wait, manage, and return the `{ client, url, container, host, port }`
  trio.
- Private dependencies: a `[plugins]` table in the PLUGIN'S own prova.toml (e.g.
  `inner = { path = "deps/inner" }`) — isolated from consumers, no version bleed.
- Ship `library/<name>.lua` (`---@meta <name>`) — it is what makes your API answerable in
  editors; lint warns without it.

## Where plugins live, nearest first

| Stage | Where |
|---|---|
| Package-local (this repo only) | a dir under `plugin_root` — requirable by name, zero declaration |
| Shared, pinned | its own repo; consumers declare `[plugins] name = "owner/repo@ref"` |
| A local file while incubating | `[plugins] name = "./plugins/name.lua"` or `-P name=./...` |

Self-test it like any package: `prova` inside the plugin repo runs its proofs against its own
namespace. Consumers' `[requires] prova` gate protects them from your future breakage.

Go deeper: `prova learn plugins` (the consumer side) · `prova learn doubles` (the mock facet).
