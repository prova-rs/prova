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

## Two doors, and they are not the same door

A topology has exactly two consumers, and each enters differently:

- **A test** builds one in-process with `prova.topology(...)`. That is a fixture — local to the
  files that declare it, and never addressable from outside the run.
- **The inhabited verbs** (`up`/`start`/`watch`/`ps`) stand up a **registered** factory, resolved
  from `[topologies]` in the manifest and **nowhere else**. They do not load or scan proof files.

So `prova.topology("orders", ...)` sitting in a test file is *not* visible to `prova up orders` —
declaring a fixture inside a test file means it belongs to that test, not that it is a shared
environment. To get both verbs, export the factory from a package and register it; a test then
builds that same factory as its fixture:

```toml
[topologies]
orders = { package = "kitchen", topology = "orders" }
vm     = { package = "parallels", topology = "vm", options = { image = "ubuntu-24.04" } }
```

```lua
local orders = prova.topology("orders", require("kitchen").orders)   -- the same factory
```

One definition, addressed twice — they cannot drift, and registering does not collide with
declaring. `options` is passed as the factory's second argument. A topology registered this way
also inherits the `requires` its package advertises, so it gates on the environment it needs.

**`startup = "15m"`** declares how long this topology needs to come up: `prova start` waits that
long for it to register (default 300s, `--timeout` overrides for one invocation). The definition
knows its own cost — a kind cluster with eight rollouts is honestly minutes — and a budget that
expires stops the holder gracefully, so whatever it already created is torn down rather than
orphaned.

## The verbs over the same definition

| Verb | Holds it |
|---|---|
| `prova up orders` | live, attached: prints endpoints, Ctrl-C tears down |
| `prova start orders` / `prova down orders` / `prova ps` | detached across processes |
| `prova watch orders` | re-applies on definition change (the dev loop) |
| MCP `up { name }` → `run`/`eval` `{ topology = name }` → `down { name }` | WARM inside the server — millisecond re-runs while iterating; see `prova learn mcp` |
| `prova up <git-url>` | stand up a topology a remote repo advertises |

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

See also: `prova learn fixtures` (a topology IS a fixture) · `prova learn doubles` (what it stands
up) · `prova learn running` (selecting and holding one) · `prova learn capabilities` (what the
host must provide)
