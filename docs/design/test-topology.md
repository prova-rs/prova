# Test Topology — what runs, where it lives, and what a green means

Drafted 2026-07-16, from two learnings that turned out to be the same learning. Companion to
[suites.md](suites.md) (the grouping model), [ide-and-layout.md](ide-and-layout.md) (the home), and
[agent-ergonomics.md](agent-ergonomics.md) (§3, the un-Dockerable resource — the case that forces
all of this).

## The principle

> A **pass** is a claim about the code. A **skip** is a claim about the environment.
> Never let the second masquerade as the first.

A skipped test is not a passed test. It is an **unanswered question** — a hole in the bar, in the
exact shape of the thing you could not ask. Today prova exits 0 either way, so "we answered
everything" and "we could not ask anything" are the same green.

This is not a new disease. It is [the false-ready](topologies.md#readiness-is-a-contract-done) one
level up: `wait = { port }` reported ready by asking a question that could not fail, and a suite
reports green by counting tests that never ran. Both are a signal that cannot say no.

## Measured, not assumed

Every claim below was run against `target/debug/prova`, because the one friction in
[agent-ergonomics.md](agent-ergonomics.md) that turned out to be false was the one filed from
reading prose (§4, retracted the same day). A scratch project with a `crates/cli` and a
`services/orders` suite:

| Question | Answer today |
|---|---|
| Does cwd scope the run? | **No.** From `crates/cli`, `orders`'s tests still run. |
| Is a selection typo caught? | **No.** `-k thisdoesnotexist` → `0 passed`, **exit 0**. |
| Is an all-skipped run distinguishable from a passing one? | **No.** `1 skipped`, **exit 0**. |
| Is `requires` docker-specific? | **No.** Unknown names fall through to a **binary-on-PATH** probe, so `requires = { "kind" }` / `{ "kubectl" }` / `{ "mvn" }` already work. |

The last row is the load-bearing good news: the capability vocabulary is **already open and
capability-generic**. Nothing below needs a new detector.

## Two axes that must not share an outcome

`-k` and `requires` both end in "this test did not run", and that is the whole bug.

| | asks | not-running means | silence is |
|---|---|---|---|
| **Selection** (`-k`, `--tags`, `--node`) | *what do I want?* | you asked for less | **correct** |
| **Capability** (`requires`) | *what can run here?* | you could not ask | **a hole** |

Selection is **intent**; capability is **ability**. A deselected test is a non-event. A skipped test
is a question the run failed to answer, and whether that is acceptable is a property of **where you
are**, not of the test.

## The contract: `requires` states a need, the profile states a guarantee

The seam already exists in this codebase, and it is the same one
[topologies.md](topologies.md#port-modes--external-reachability-done) uses for port modes: **the
definition is decoupled from the verb.** The topology does not know about `--fixed`; the verb picks
the strategy. Likewise:

- The **test** declares a **need** — `requires = { "docker" }`. A portable fact about the test.
  It is true on a laptop, in CI, and on a machine that has never heard of Docker.
- The **profile** declares a **guarantee** — `must_run = [ … ]`. A policy about the environment.
  It changes when you move; the test does not.

```toml
# The capabilities THIS context guarantees. A guaranteed capability that is
# absent is a broken environment — fail, do not skip.
[profiles.ci]
must_run = ["docker"]                    # CI is a linux runner with a daemon

[profiles.local]
must_run = ["docker", "kind", "mvn"]     # my machine owes me my whole toolchain
```

`must_run` is **generic over the whole capability vocabulary** — the same names `requires` uses, with
the same probes. `must_run = ["kind"]` means kind must be on PATH. There is no privileged capability.

### `must_run` is a precondition, not a skip-audit

Check the guarantees **before running anything**, and fail with the probe's own answer:

```
FAIL  profile 'ci' guarantees capability 'docker', which is unavailable
      └ docker info --format {{.OSType}} → "windows" (prova's resources are linux containers)
```

Fail-fast, for three reasons: the message is actionable at second one rather than minute ten; a CI
runner that silently lost its daemon is caught before it wastes the run; and a precondition is a
*simpler concept* than auditing which skips were forgivable. Post-hoc skip attribution still gets
reported — but the gate is the precondition.

### What this buys, concretely

- **Locally**: `kind`/`mvn`/`cargo` suites run, because your profile guarantees them. If your kind
  cluster is down you find out immediately instead of getting a green that means nothing.
- **In CI**: docker suites **must** execute and pass. Your local-flow tools are not guaranteed, so
  their suites skip — *by design, declared, visible* — instead of failing or lying.
- **On Windows**: the honest answer, out loud. `must_run = ["docker"]` on a Windows-container daemon
  fails with "prova's resources are linux containers", so the choice — don't run this profile here,
  or declare the platform unsupported — is made explicitly instead of by a green that skipped
  everything. (This is live: as of the v0.2.7 capability fix, prova's own Windows leg would otherwise
  go green with every docker test skipped.)

### Empty selection is an error

`-k thisdoesnotexist` → `0 passed`, exit 0, today. A selection that matches nothing is nearly always
a typo, and a typo must not be green. Explicit selection matching zero nodes is an error;
`--allow-empty` opts out for the matrix case that legitimately selects nothing.

This is the *selection*-axis instance of the same principle: a run that asked nothing must not report
success.

## Home is the anchor, not the container

The second conflation. Today the home answers two questions it should not:

> **Home = where config comes from** (walk **up**, like git finding `.git`).
> **CWD = what runs by default** (walk **down**).

`ide-and-layout.md` already defines home precisely — the directory containing `prova.toml`, with
`root` (the project root the editor binds to) and `dir` (`root`, or its `prova/`/`.prova/` child).
`prova.root` / `prova.home` now surface both to authors. What is missing is only that **home also
decides what runs**, so `cd crates/prova-cli && prova` runs the entire repo. Splitting them is the
cargo/pytest idiom and needs no new concept — just a default:

```
$ prova                      # from the root: every suite under [run] paths
$ cd crates/prova-cli
$ prova                      # THIS subtree's suites; config still from the one home
$ prova --all                # override: the whole project from anywhere
```

**Exactly one home per project.** Not a nested-manifest workspace: one `[plugins]` table, one
`annotations/`, one `.luarc.json` question, one place to look. Module autonomy is expressed by
**suites**, which are already directory-aligned — not by a second manifest.

## The canonical layout

Nothing here is new machinery; it is the existing pieces, named:

```
<repo>/
├── .luarc.json                  ← the only root clutter (LuaLS binds here)
├── prova/                       ← THE home: the project's anchor
│   ├── prova.toml               #   [plugins], [profiles.*] + must_run, [run] paths
│   ├── plugins/ourthing.lua     #   locally-authored plugins
│   ├── annotations/             #   GENERATED, self-gitignored
│   └── suites/                  #   project-level suites (cross-cutting)
├── crates/prova-cli/
│   └── prova/
│       ├── suite.lua            ← a SUITE (one Lua state, its own Scope.Suite fixtures)
│       └── cli_test.lua
└── services/orders/
    └── prova/
        ├── suite.lua            ← a sibling suite; runs in parallel with the above
        └── crud_test.lua
```

The rule that resolves "one directory or per-module?" — **it was two questions**:

- **The home is singular** because config must be unambiguous.
- **Suites are plural and local** because a suite is a Lua state and a state belongs next to what it
  is testing.

So `<repo>/prova/` is not where tests must live; it is where the *project* is declared. Tests live
where their code lives, and `cd` there to run them. Both of the user's instincts were right; they
just answer different questions.

## What this changes in `suites.md`

One line, and it is the doc's own cascade:

> *"A suite that `requires` an unmet capability skips **all** its files (cascade), reported once."*

The cascade stays — it is right, and reporting once is right. What changes is its **standing**: a
cascaded skip is a **declared hole**, and in any profile that guaranteed the capability it is a
**failure**, caught by the precondition before the suite is even loaded. A whole suite quietly
evaporating is the largest vacuous green available, and it is currently the cheapest one to produce.

`suite.config{ requires = … }` needs no change: it is the same axis, at the suite grain.

## Why `local_service` belongs to this arc

[agent-ergonomics.md §3](agent-ergonomics.md) is the reason this contract has to exist rather than
being a tidiness exercise. Minion is a Resource in every way that matters — provision, wait, manage,
return a client — and it **cannot be containerized**: it needs macOS, TCC grants, real HID devices.
It is un-Dockerable *in principle*, "because the thing under test **is** the local machine's
integration."

That generalizes. `kind`, `mvn`, a GPU, a signing key, a physical device: a permanent population of
tests that **can never run in CI**, and that is not a gap to close. If the only shape prova blesses
is `containerized`, the temptation is to declare CI the whole bar and quietly lose them. The contract
above is what lets them exist honestly: they say `requires = { "macos" }`, the local profile
guarantees it, CI does not, and the skip is a *declared* absence rather than a lie.

So the constructor is designed with the contract, not after it:

```lua
local minion = prova.local_service{
  name   = "minion",
  bin    = prova.root .. "/target/debug/miniond",   -- prova.root: the anchor §2 asked for
  env    = function(dir) return hermetic_env(dir) end,
  url    = function(sock) return "unix://" .. sock end,
  client = function(url) return connect(url) end,
  wait   = { … }, timeout = "20s",
}
local svc = minion.service(ctx)     -- { client, url, handle } — handle:stop() like a container
```

It is `containerized`'s body with `shell.spawn` where `docker.run` was, which is the doc's own test
for a new constructor ("a shape proves to carry recurring boilerplate"). **Implementation still
waits for the second local-daemon plugin** — one case is a shape, two is a pattern — but the *shape*
is designed now, because a skip/fail contract drafted against Docker alone would fit only Docker.

Third-slot naming (`{ client, url, handle }` with `container` kept as the Docker-case alias) is
§3(a), and lands with it.

## Open

- **The capability vocabulary is open by design** (binary-on-PATH fallback), which makes
  `requires = { "kubectl" }` work with no registration — and makes `requires = { "dokcer" }` skip
  silently forever. Measured: a typo'd capability skips, exit 0. `must_run` covers the capabilities a
  context cares about; whether typos deserve more (a warned-on known-name set) is deferred — the open
  vocabulary is worth more than the typo protection.
- **Registry + plugin browser + `prova init` placement** — a companion arc, not this doc: a
  user-level registry config, `prova plugin search/add`, and MCP tools so an agent can do it. One
  constraint up front: an agent adding a plugin is a **supply-chain action**, so user consent is part
  of the design, not a nicety.
