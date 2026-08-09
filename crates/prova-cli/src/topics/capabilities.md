# capabilities — what the host can do, and what a test needs it to

A **capability** is a fact about the *environment*, not the code: a daemon that answers, a tool on
`PATH`, a client compiled in, an OS. Tests state what they need; prova checks it against the host.

```
prova capabilities        # the built-in vocabulary, each MET or UNMET on THIS machine (and why)
```

## The vocabulary

- **Named host probes:** `docker` (a daemon that answers *and* runs Linux containers — not just
  `docker info`), `github` (`GITHUB_TOKEN` present), `unix` / `windows` (this OS), `network` /
  `internet`.
- **Native clients** (compiled into this build): `http`, `sqlite`, `grpc`, `graphql`, `yaml`.
- **Anything on `PATH`:** an unknown name is a binary probe — `requires = { "kubectl" }` just works,
  no registration.
- **Registered:** a package adds its own with `runtime.capability("gpu", …)` in `prova.lua` — a
  fact prova cannot probe (a license, a device), reported by the project itself.

A capability is an *expression*, so a version can ride along: `"docker"`, `"dotnet >= 9"`,
`"node ^20"`. The same parser serves both directions below — one vocabulary, never two.

## Two directions: requires (skip) and must_run (fail)

- `requires = { "docker" }` on a test/suite/topology — **skip** when unmet. Graceful degradation:
  the box without docker runs everything else and stays green.
- `[run] must_run = ["docker"]` / `[profiles.ci] must_run = [...]` — **fail** when unmet. A named
  profile's *guarantee* about its environment: `prova run ci` on a box missing docker is a broken
  environment (exit 2), not a quiet skip. Only bare `prova` is opportunistic; a profile is a contract.

The one rule both obey: an unmet capability is never silently "green". `requires` skips loudly (the
reason is in the report); `must_run` fails loudly. A typo'd constraint is a config **error**, not a
skip — a gate that never matched would read as passing, the vacuous green this contract exists to remove.

## Not the same as a package's "capabilities"

A package advertises descriptive `capabilities` tags for `prova packages <query>` search — discovery
metadata ("this package speaks kafka"). That is a *label*, unrelated to whether **your** host can do
a thing. `prova capabilities` is the host check; the registry field is a catalog keyword.
