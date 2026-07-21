# mcp — driving prova as an MCP server

`prova mcp` serves stdio MCP; tools mirror the CLI and return one text item of compact JSON
(exception: `learn`, which returns markdown). The embedded skill arrives as `instructions`, so
a connected agent starts knowing the loop. The server resolves the package from its working
directory once, at startup — but any call can retarget with `package` (a directory or manifest
path), which resolves FRESH, so a package you just scaffolded works without a restart.

| Tool | CLI twin | Notes |
|---|---|---|
| `run { keywords?, tags?, nodes?, last_failed?, profile?, jobs?, topology?, package? }` | `prova -k/--tags/--node/--last-failed/--profile/-j` | isError on any failure; failures carry `{ path, message }`; records last-failed |
| `list { same selection, package? }` | `prova --list` | `{ nodes: [{ path }] }` |
| `eval { code, topology?, package? }` | `prova eval` | full environment, real ctx, auto-teardown |
| `learn { topic?, package? }` / `introspect { filter? }` | `prova learn` / `prova.help()` | the knowledge surface |
| `up { name, package?, fixed? }` / `down { name }` / `status {}` | `prova up/down/ps` | held INSIDE the server (below) |

Tools serialize FIFO — side-effects land in the order you call them.

## Warm re-runs: the reason to prefer MCP while iterating

```
up { name = "orders" }                  -- provision ONCE, hold it in the server
run { topology = "orders", last_failed = true }   -- millisecond re-runs, no re-provisioning
eval { code = "return orders.db.url", topology = "orders" }  -- held value is a global
down { name = "orders" }                -- the ONE place teardown happens
```

- Warm calls NEVER provision implicitly — an un-held topology is an explicit error; `up` first.
- The holder owns teardown: warm runs never reap; `down` (or server shutdown) does.
- Held state accumulates on purpose; `down` + `up` when isolation matters.
- `status {}` lists what's held with endpoints.

## Split the work across the two surfaces

| Do over MCP | Shell out to the CLI |
|---|---|
| iterate: run/eval, warm topologies | `prova init` (scaffolding), `prova ide setup` |
| discovery: learn, introspect | `prova plugin lint`, `prova skill --install` |
| targeting other packages via `package` | anything CI runs (CI is always the CLI) |

Topics are also protocol-native resources: `prova://learn/<topic>` and `prova://skill`, same
content as the `learn` tool.
