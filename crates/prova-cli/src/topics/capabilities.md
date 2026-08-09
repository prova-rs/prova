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

## Not a capability: the opt-in switch

"Someone asked for this expensive class" is **intent**, not a host fact — so it is never a
capability. A test that must not fire unasked carries `switch = "<class>"` (off unless thrown
with `-s <class>` or a profile's `switches = [...]`), and keeps `requires` for what the WORLD
must provide. Two facts, two remedies: `prova run ut` on a box without nextest fails the
`must_run` guarantee (install it); bare `prova` simply holds the class back (throw it when you
mean it). Registering an env-var-probing capability to gate a test class is the old pattern this
replaced (docs/design/manifest.md#switches-not-env-capabilities).

## One meaning — the registry uses `keywords`

"Capability" names exactly this: a host fact a test needs. It is deliberately NOT the registry's
discovery vocabulary — a package advertises `keywords` for `prova packages <query>` search
("kafka", "messaging"), pure catalog metadata unrelated to whether *your* host can do a thing. What
a package needs from the host still lives in its `requires` (the same words as here). So the word
has one job: `prova capabilities` and `requires`/`must_run` probe the host; `keywords` find a package.
