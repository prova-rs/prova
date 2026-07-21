# plugins — using capabilities the core doesn't ship

A plugin is a Lua namespace you `require("<name>")` in proofs. Declare where each name comes
from in `prova.toml`; sources are pinned in-repo, so local runs and CI resolve identically.

```toml
[plugins]
postgres = "prova-rs/prova-postgres@v1"                        # owner/repo@ref (ref REQUIRED)
greet    = "./plugins/greet.lua"                               # local file
support  = { path = "./test-support" }                         # local dir (init.lua)
rabbitmq = { git = "https://github.com/acme/prova-rabbitmq", tag = "v1.0.0" }
nats     = { git = "…", rev = "abc123", module = "src/nats.lua" }

[sources]                       # alias → base, so teams shorten their own hosts
acme = "github:acme"            # then: plugin = "acme:prova-redis@v1"
```

## In this package

{{plugins}}

{{plugin_root}}

## What a resource plugin gives you — the facet grammar

Every service namespace has the same shape, so knowing one is knowing all:

- `X.client(url_or_opts)` — attach to something already running.
- `X.container(ctx, opts?)` — provision the real thing ephemerally →
  `{ client, url, container, host, port }`.
- `X.wait_for(...)` — readiness probe.
- `X.mock(ctx, opts?)` — where mocking that transport makes sense (see `prova learn doubles`).

Official plugins: postgres, mysql, redis, kafka, pulsar, rabbitmq, s3, mongodb, parallels.
Built-ins need no declaration: `fs shell net http grpc graphql yaml sqlite docker archetect`.

## Operational knobs

- Ad-hoc, no manifest edit: `-P name=source` (repeatable; local paths; layers over `[plugins]`).
- Profile-scoped: `[profiles.ci.plugins]` overlays `[plugins]` (profile wins on conflict) — CI
  capabilities stay pinned in-repo.
- Git freshness: cached under the user cache; `[updates] interval = "1d"` gates re-checks;
  `-U/--update` forces; `--offline` never fetches.
- A plugin's API is discoverable: `prova.help("<name>")` once its stub is synced (IDE gets it
  automatically), or probe with `prova eval`.

No plugin for your dependency? Compose primitives: `docker.run{ image, env, ports, wait }` +
`container:run(argv)` + `prova.retry`. When the boilerplate recurs, promote it:
`prova learn plugin-authoring`.
