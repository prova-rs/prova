# topologies — one environment definition, every verb

A topology is a named factory for a whole environment. Define it once; tests use it, dev holds
it live, CI provisions it fresh — one description, so they cannot drift.

```lua
local env = prova.topology("orders", function(ctx)
  local db = require("postgres").container(ctx)
  db.client:execute("create table orders (id int, sku text)")
  return { db = db }
end)

prova.test("reads through the stack", function(t)
  local e = t:use(env)                     -- in a test: it's a fixture
  ...
end)
```

## The verbs over the same definition

| Verb | Holds it |
|---|---|
| `prova up orders` | live, attached: prints endpoints, Ctrl-C tears down |
| `prova start orders` / `prova down orders` / `prova ps` | detached across processes |
| `prova watch orders` | re-applies on definition change (the dev loop) |
| MCP `up { name }` → `run`/`eval` `{ topology = name }` → `down { name }` | WARM inside the server — millisecond re-runs while iterating; see `prova learn mcp` |
| `prova up <git-url>` | stand up a topology a remote repo advertises |

Manifest-registered topologies name a plugin's factory (with `options` passed as the factory's
second argument):

```toml
[topologies]
vm = { plugin = "parallels", topology = "vm", options = { image = "ubuntu-24.04" } }
```

## In this package

{{topologies}}

## The network vantage — the classic mistake and its fix

Inside a topology factory (and ONLY there) `ctx.network` is an ambient managed network:
resources auto-join, aliased by recipe name, and each gets TWO addresses —

- `res.url` — 127.0.0.1 + mapped port: what the TEST RUNNER dials.
- `res.network` = `{ url, host, port, alias }` — alias + container port: what IN-NETWORK
  consumers (a containerized SUT) dial.

Wiring a container to a resource's host `url` is the classic mistake: inside a container,
`127.0.0.1` is that container. Hand the SUT `db.network.url`; probe it yourself over `app.url`.

A held environment accumulates state — that's the point; `down` then `up` when isolation
matters.

Go deeper: `prova learn fixtures` (scopes underneath) · `prova learn doubles` (what goes in it).
