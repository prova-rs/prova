# Module Namespacing

Decided 2026-07-13. This document records the naming grammar for Prova's Lua module surface —
the rule every existing module follows and every future module (first-party or plugin) must follow.

## The grammar

**One global namespace per technology you talk to.** `postgres`, `mysql`, `sqlite`, `redis`,
`kafka`, `pulsar`, `s3`, `grpc`, `graphql`, `http` — not grouping modules like the old `db`.
A namespace is the unit of everything: one Cargo feature, one registration in `modules.rs`, one
section in `library/modules.lua`, one reference page in the docs.

**Fixed facet names within every service namespace:**

| Facet | Meaning |
|---|---|
| `X.client(...)` | Attach to something already running (a CI service, a live environment, a locally spawned process). Named for what it returns, not what it does — some clients handshake at construction (kafka, grpc), some don't (http); the test author shouldn't care. |
| `X.container(ctx, opts?)` | Provision an ephemeral instance via Docker, wait for readiness, attach a managed client, tie teardown to the scope. Sugar over `docker.run` + `prova.retry` + `X.client` + `ctx:manage`. |
| `X.wait_for(...)` | Readiness polling, where the protocol supports a cheap probe (http, grpc). |
| `X.mock(ctx, opts?)` | Provision a **fake** instance — a real server, in-process, that you stub and then assert on. Where `container` provisions the real thing, `mock` provisions a stand-in for the thing you *can't* run. Returns the resource shape plus a request journal. |

Facets are optional per namespace: `sqlite` has no `container` (nothing to provision);
`http`/`grpc`/`graphql` are protocol namespaces with no `container` either — and `mock`
is the mirror case, meaningful only where a *protocol* can be served (`http`, `grpc`) or
where a plugin virtualizes a specific SaaS (a `stripe` plugin's `stripe.mock(ctx)`). You
would never mock `postgres`; you would run it.

The pairing is the teachable part: **`client` attaches to a real one, `container` provisions
a real one, `mock` provisions a fake one.** Reach for `mock` only on a boundary you cannot
run, for behavior the real thing won't produce on demand, or to assert on the *interaction
itself* — see `docs/plans/mocks.md` for why that scope is deliberately narrow.

**One standard resource shape.** Every `X.container` returns:

```lua
{ client = <what X.client returns>, url = <string that reaches the instance>, container = <docker handle> }
```

- `client` is the *same type* `X.client()` returns.
- `url` is the connection string you inject into the app under test's env (bootstrap brokers for
  kafka, endpoint URL for s3).
- Extra per-tech fields are allowed (`s3` adds `access_key`/`secret_key`), but the trio is guaranteed.

The teachable summary: *"`client` to attach, `container` to provision; every resource has
`.client`, `.url`, `.container`."*

## What this replaced

| Old | New |
|---|---|
| `db.connect(url)` (URL-scheme dispatch) | `postgres.client(url)` / `mysql.client(url)` / `sqlite.client(url)` |
| `db.postgres(ctx, opts)` / `db.mysql(ctx, opts)` | `postgres.container(ctx, opts)` / `mysql.container(ctx, opts)` |
| `redis.connect` / `kafka.connect` / `pulsar.connect` / `grpc.connect` | `…client` |
| `s3.connect{ endpoint = … }` | `s3.client{ url = … }` |
| Resource fields `conn` / `brokers` / `endpoint` / `bucket` | `client` / `url` |
| Cargo feature `db` | Features `postgres`, `mysql`, `sqlite` |

The three SQL namespaces still share one generic `Connection` type (sqlx `Any` driver) — the
unification survives as a *type*, not a namespace. Each engine's `client` validates its URL scheme,
so `postgres.client("mysql://…")` fails with a clear message instead of a driver error.

## Why technology-first

- **Discoverability**: typing `postgres.` in an editor lists everything Postgres-related. Under
  `db`, Postgres was invisible until you knew to look inside.
- **Growth**: a new tech (mongo, nats, elasticsearch, rabbitmq…) never poses a "which module does
  it belong to?" question — it gets its own namespace with the standard facets, and readers already
  know its shape before opening the docs.
- **Plugins**: `archetect` already works this way (a plugin = a namespace). Third-party plugins
  inherit the grammar for free.

## Rules for new modules

1. Namespace = the API you speak. `container` provisions a *default implementation* of that API
   (`s3.container` runs MinIO); `image`/`tag` opts override it.
2. `client` before sugar: land the attach path first; `container` composes it with `docker.run`.
3. Resource extras are fine; renaming the trio is not.
4. `opts` keys prefer the grammar's vocabulary (`url`) over vendor vocabulary (`endpoint`) when
   the meaning is the same.
