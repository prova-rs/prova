# packages — using capabilities the core doesn't ship

Everything is a package: your project, the libraries it depends on, the topologies they
advertise — one `prova.toml` shape, worn different ways. A dependency is a package you
`require("<name>")` in proofs. Declare where each name comes from in `prova.toml`; sources are
pinned in-repo, so local runs and CI resolve identically.

```toml
[dependencies]
postgres = "prova-rs/prova-postgres@v1"                        # owner/repo@ref (ref REQUIRED)
greet    = "./packages/greet.lua"                              # local file
support  = { path = "./test-support" }                         # local dir (init.lua)
rabbitmq = { git = "https://github.com/acme/prova-rabbitmq", tag = "v1.0.0" }
nats     = { git = "…", rev = "abc123", module = "src/nats.lua" }

[sources]                       # alias → base, so teams shorten their own hosts
acme = "github:acme"            # then: package = "acme:prova-redis@v1"
```

## In this package

{{packages}}

{{packages_dir}}

## What a resource package gives you — the facet grammar

Every service namespace has the same shape, so knowing one is knowing all:

- `X.client(url_or_opts)` — attach to something already running.
- `X.container(ctx, opts?)` — provision the real thing ephemerally →
  `{ client, url, container, host, port }`.
- `X.wait_for(...)` — readiness probe.
- `X.mock(ctx, opts?)` — where mocking that transport makes sense (see `prova learn doubles`).

Official packages: postgres, mysql, redis, kafka, pulsar, rabbitmq, s3, mongodb, parallels.
Built-ins need no declaration: `fs shell net http grpc graphql yaml sqlite docker archetect`.

## Finding more — search the registries

{{registries}}

## Operational knobs

- Ad-hoc, no manifest edit: `-P name=source` (repeatable; local paths; layers over
  `[dependencies]`).
- Profile-scoped: `[profiles.ci.dependencies]` overlays `[dependencies]` (profile wins on
  conflict) — CI capabilities stay pinned in-repo.
- Git freshness: cached under the user cache; `[updates] interval = "1d"` gates re-checks;
  `-U/--update` forces; `--offline` never fetches.
- A package's API is discoverable: `prova.help("<name>")` once its stub is synced (IDE gets it
  automatically), or probe with `prova eval`.

No package for your dependency (registries searched)? Compose primitives:
`docker.run{ image, env, ports, wait }` + `container:run(argv)` + `prova.retry`. When the
boilerplate recurs, promote it: `prova learn package-authoring`.

See also: `prova learn package-authoring` (publishing one) · `prova learn project` (where deps are
declared) · `prova learn doubles` (the resources they provide)
