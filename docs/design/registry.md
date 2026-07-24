# Plugin Registry

Drafted 2026-07-24. The discovery layer for the plugin ecosystem — how an agent (or a human) finds
plugins it does not yet know exist, and adds them with a pinned manifest entry. This is roadmap
item 7 of [ecosystem.md](ecosystem.md) ("the registry index, when the count earns it") made
concrete. Where [plugin-system.md](plugin-system.md) answers *how a declared plugin loads* and
ecosystem.md answers *how the ecosystem is tiered*, this answers *how you learn what exists*.

> Why this matters for the *practice*: prova is agent-centric. An agent asked to prove a system
> against postgres should not hand-write a postgres recipe — it should ask "is there a plugin for
> this?", get an answer from the binary, and go from search → declared → in use in one motion.
> Discovery is what makes the ecosystem reachable from inside a session, the same way
> `prova init --list` makes templates reachable and `prova up` makes topologies reachable.

## What a registry is

**A registry is a git repository containing one TOML file per plugin.** Nothing more — no server,
no API, no database. Prova fetches it through the same pinned/freshness-gated git cache every
plugin source already uses, parses the entries into memory, and answers list/search queries
locally.

```
package-registry/
  registry/
    postgres.toml
    rabbitmq.toml
    kafka.toml
    ...
  proofs/            # the registry proves itself — see Automation
  prova.toml
```

Users list registries in the user-level config (`~/.config/prova/config.toml`); the `prova-rs`
registry (`prova-rs/package-registry`) is built in as the default, and user entries add to or
override the built-in set by name — the same `Catalog::builtin()` + merge pattern the init catalog
established:

```toml
[[registries]]
name   = "acme"
source = "https://github.com/acme/prova-registry"
```

**Trust is org-granularity.** Listing a registry means trusting the organization that publishes
it: an agent is then free to search it and add its plugins while building proofs, without a
per-plugin approval ceremony. The gate an org actually controls is downstream anyway — every add
lands as a pinned diff in `prova.toml`, reviewed like any other change.

### DECISION (2026-07-24): one TOML file per entry, in git — not sqlite, not a single index file

The format was chosen for three properties, in priority order:

1. **CI-writable.** Registration is automation, not a human editing an index (see Automation
   below). One-file-per-plugin means concurrent registrations never merge-conflict, an entry's
   removal is a file delete, an update touches exactly one file, and every registration is a
   self-describing, reviewable diff. A single JSON/TOML index file fails this — every writer
   contends on one file; a sqlite database fails it completely — binary blobs cannot be diffed,
   reviewed, or written by a `sed`-grade CI step.
2. **Gracefully extensible.** Readers ignore unknown keys (the same tolerance `UserConfig`
   already practices), so entries can grow fields without breaking older binaries. Each entry
   carries `schema = 1`; a reader skips entries whose major schema it does not understand — with
   a warning naming the entry — rather than failing the whole registry. Old binary, newer
   registry: degraded per-entry, never broken.
3. **In-memory scale is the actual scale.** A registry holds at most a few hundred entries;
   parsing a few hundred small TOML files is microseconds. A database earns its complexity at
   thousands of entries with partial reads — a bar this will not meet. Prova has no internal
   database today (the `sqlite` global is a *proof module* for users, not infrastructure), and
   the registry is not the feature that introduces one.

TOML over JSON because it is the house format (`prova.toml`, `config.toml`), comments are legal
(automation can stamp provenance), and entries stay hand-authorable for the PR path.

## The entry

```toml
# registry/postgres.toml — written by automation; see Automation below
schema       = 1
name         = "postgres"
repo         = "https://github.com/prova-rs/prova-postgres"
description  = "Postgres containers, seeded schemas, and direct SQL assertion via psql-in-image"
capabilities = ["postgres", "sql", "database", "container"]
latest       = "v2"

# optional — derived from the plugin's own manifest at registration time
namespaces = ["postgres"]        # what require() will return
topologies = []                  # topologies the plugin advertises
shapes     = ["resource"]        # resource | library | client | composite (plugin-shapes table)
requires   = ["docker"]          # capability gates the plugin declares
```

`name`, `repo`, and `description` are required; everything else optional. `capabilities` is the
search surface — free-form terms an agent would reach for ("postgres", "database", "queue",
"jwt"), matched together with name and description. `latest` is the **recommended pin**, not a
constraint: it is what `prova plugins add` writes into the manifest when no `@ref` is given.

**The plugin repo is the source of truth for its own entry.** Automation derives the entry from
the plugin's manifest (`[plugin]` section, advertised topologies) and `prova plugin lint`'s shape
classification — the registry entry is a projection, never hand-maintained metadata that can
drift.

## Discovery-only: the registry never resolves anything

[plugin-system.md](plugin-system.md) draws a hard line: the user-level config *"may change how
prova presents things … it may never change what prova resolves."* Registries are listed in that
config — so the registry must sit entirely on the presentation side of the line, and it does:

- **Search and list read the registry.** `require` never does. No name resolves through a
  registry at run time; the searcher's no-network safety boundary is untouched.
- **Adding a plugin materializes a pin.** `prova plugins add postgres` looks the name up across
  configured registries and writes the ordinary, explicit entry into `prova.toml`:

  ```toml
  [plugins]
  postgres = { git = "https://github.com/prova-rs/prova-postgres", tag = "v2" }
  ```

  From that moment the registry is out of the picture: the manifest is the canonical, committed,
  pinned source of truth (ecosystem.md's standing rule), and a fresh checkout reproduces the run
  with **zero registries configured**. Delete every registry from your config and nothing that
  ran yesterday changes today.

This also settles how the resolution ladder's tier 5 (`redis = "^1.2"`) must eventually work, if
it is ever built: version-range resolution would happen at **add/update time** (a future
`prova plugins update` re-pinning the manifest), or against a registry named **in the manifest
itself** — never implicitly via user config at require time. The committed file always tells the
whole story.

## Surface

Rhymes with the existing discovery pair — `prova init --list` for templates, `prova up` (no-arg)
for topologies:

```bash
prova plugins                    # list all entries across configured registries
prova plugins postgres           # search: name + description + capabilities substring match
prova plugins info postgres      # one entry, full detail (namespaces, shapes, requires, latest)
prova plugins add postgres       # write pinned [plugins] entry (latest) into prova.toml
prova plugins add postgres@v1    # explicit ref wins over latest
```

Search is a dumb in-memory match over a few hundred entries — no query language, no ranking
beyond name-hit-first. Output is the same key-column `name  description` rows the init catalog
prints, with the registry name shown when more than one registry is configured (a name present in
two registries lists both; `add` requires disambiguation as `registry:name`).

The MCP surface mirrors the verbs (the same shared implementation, `Transport::{Cli,Mcp}`
changing only the spelling of suggested next moves, per the learn system's rule).

### The learn system announces it

Discovery that agents don't know about doesn't exist. The autodidact system is the delivery
vehicle (this is the registry slot [autodidact.md](../plans/autodidact.md) deferred):

- A `Slot::Registries` renders the configured registries and entry counts into the relevant
  topics (`plugins`, `project`), with the standing instruction: **before hand-writing a
  capability, search the registries** — `prova plugins <term>`.
- The flow the topic teaches: need postgres → `prova plugins postgres` → read capabilities →
  `prova plugins add postgres` → `require("postgres")` in the proof. Search to in-use, no human
  in the loop beyond the trust already granted by listing the registry.

## Caching and freshness

Exactly the plugin rules, reused verbatim — a registry is just another mutable git source:

- Fetched through `archetect-git-cache` into `~/.cache/prova`, default-branch (mutable) pin.
- Refresh is TTL-gated by `[updates] interval` (default 1 day); past the interval a cheap
  `git ls-remote` hash check decides whether to pull. `-U`/`--update` forces; `--offline` serves
  the cache and never touches the network.
- Entries are parsed on demand and held in memory for the invocation. No secondary index, no
  local database — the git checkout *is* the cache.

A search against a stale-but-cached registry is correct behavior, not an error: the entry's
`repo` is canonical and `add` pins from it, so the worst staleness cost is not seeing a plugin
registered since the last refresh — cured by `-U`.

## Automation: registration is CI, not curation

The registry repo maintains itself; humans review, they don't type entries. Three paths in,
one path out:

- **`workflow_dispatch` — the register verb.** Inputs `{ repo, ref }`. The workflow checks out
  the plugin repo at `ref`, derives the entry (manifest `[plugin]` section + `prova plugin lint`
  shape classification), writes `registry/<name>.toml`, commits. Idempotent upsert — re-dispatch
  updates the entry in place.
- **Org events — plugins register themselves.** Two complementary mechanisms (both landed
  2026-07-24): a **reconcile loop** in the registry repo — scheduled, credential-free — pulls
  the org's state and converges the registry onto it (any `prova-rs/prova-*` repo with a
  release and a `[plugin]` manifest gets an entry at its latest release; entries whose repos
  are deleted or archived are removed), and a **release-time dispatch hop** in each plugin repo
  (`release: published` → a `repository_dispatch(register)` event on the registry) for instant
  registration. The hop rides the existing org-wide `PROVA_DISPATCH_TOKEN`: repository_dispatch
  needs only Contents: write, so no Actions scope was added to the token. If a dispatch ever
  fails, the reconcile loop still guarantees registration within its interval — the hop only
  buys latency. Proven live: hello v1.3 registered seconds after `release: published`. The org's registry tracks the org's plugins with no human in the loop.
- **Pull requests — the third-party path.** Anyone can PR an entry file into a registry they
  don't control; review of that one-file diff *is* the curation step. Same format, same
  validation, human gate instead of webhook gate.

**The registry proves itself.** The repo carries a prova proof suite (dogfood) asserting every
entry parses, required fields are present, `name` matches its filename, `repo` is a well-formed
git source, and schemas are known. The same suite gates PR merges and runs after every dispatch
commit — a registry that ships a broken entry has a red proof, like any other system prova holds
to a bar.

## Non-goals

- **No version solver.** Unchanged from ecosystem.md: plugins are flat and self-contained; the
  registry maps name → repo + recommended pin and stops. `latest` is advice for `add`, not a
  constraint to satisfy.
- **No hosted service.** A registry is a repo. Availability, auth, and mirroring are git's
  problem, already solved; `--offline` works because the cache is a checkout.
- **No `prova publish`.** Registration belongs to the registry's own automation (dispatch,
  webhook, PR) — the prova binary reads registries, it never writes one. A publish verb would
  put prova in the business of holding write credentials to other people's repos.
- **No signing/checksum layer (v1).** Integrity of the index is git's; authenticity is the org's
  (you chose to list them). The pin written into `prova.toml` is a tag today — an org wanting
  immutability pins a `rev`, and a future schema bump can add per-version revs to entries
  without breaking older readers (that's what `schema` is for).

## Roadmap

**Status (2026-07-24):** items 1–3 below are **implemented and graduated** — the
`proofs/spec/registry/` suite (16 proofs) runs flag-free: `[[registries]]` + built-in merge,
`prova plugins` list/search/info/add, per-entry tolerance, the offline/cold-cache error, the
discovery-only guardrail, and the `{{registries}}` learn slot. One deliberate deviation from the
Surface sketch: `add` pins the manifest but does NOT fetch — pinning must work offline, and the
next resolution fetches through the normal path.

**Status (2026-07-24, later):** items 4–5 are **live** — `prova-rs/package-registry` exists and
serves the built-in default: entries for every *released* org plugin (derived by
`scripts/derive_entry.py`, a projection of each plugin's `[plugin]` manifest; unreleased plugins
join on their first release), `register.yml`/`remove.yml` dispatches, the credential-free
`reconcile.yml` convergence loop (see Automation), and the self-proof suite gating PRs and
automation commits. The whole lifecycle is proven end-to-end by `prova-rs/prova-hello` (rendered
from the plugin archetype): create → release v1.0 → auto-registered → `prova plugins add hello`
→ `require("hello")` green in a consumer proof → release v1.1 → `latest` bumped → archive →
entry removed. The release-dispatch hop is wired in prova-hello
and live over the org-wide `PROVA_DISPATCH_TOKEN` (via `repository_dispatch`, which needs only
Contents: write). Remaining: the MCP mirror of the verbs, and item 6 (update ergonomics).

1. `[[registries]]` in `config.toml` + built-in `prova-rs` default; fetch/cache via the existing
   git-cache path; entry parser with unknown-key tolerance + per-entry schema skip.
2. `prova plugins` list / search / info / add (manifest edit + immediate fetch), CLI + MCP.
3. `Slot::Registries` + learn-topic teaching of the search-first flow (closes the autodidact
   deferral).
4. Stand up `prova-rs/package-registry`: entries for the existing `prova-rs/prova-<name>`
   plugins, the register/remove dispatch workflows, the self-proof suite.
5. Org webhook / reusable release workflow — the zero-touch registration loop.
6. *(later, if earned)* `prova plugins update` re-pinning, per-version revs in entries,
   `registry:name` cross-registry disambiguation ergonomics.
