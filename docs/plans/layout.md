# Plan: layout — the canonical prova package, and the manifest as a lifecycle spec

Design refs: `docs/design/suites.md` (the suite = one Lua state model), `docs/design/ide-and-layout.md`
(the "home"), `docs/design/test-topology.md` (`runtime.capability`, the companion), `docs/design/api.md`
(the original "a module that returns handles, require'd by siblings" note — this plan is that,
generalized). Dogfooding target: the repo's own `prova/` directory.

## The thesis, in one line

**`prova.toml` is a lifecycle spec; `require()` is the one sharing primitive; conventions are its
defaults.** No magic conftest file, no fixture-inheritance, no phase-gated dual-purpose companion.

## What the manifest declares — read top to bottom, know the order of operations

```toml
[run]
config  = "shared/config.lua"   # phase 1: loaded ONCE, pre-suite — runtime.* (capabilities)
preload = ["shared.global"]     # phase 2: auto-require'd into EVERY suite VM — ambient setup
paths   = ["suites"]            # discovery: suite.lua grouping + *_test.lua
```

The whole home collapses to three clean things, because files are referenced by *path*, not summoned
by a magic *name* — a magic name forces a file's location; a path frees it:

```
prova/
├── prova.toml          # the map
├── shared/             # code the suites lean on (config, ambient, fixtures, helpers)
└── suites/             # the tests
```

## The five decisions, and why

### 1. `require()` is the sharing primitive — not a magic conftest file

The reason it's not just cleaner but *correct*: **Lua's `package.loaded` is per-state.** So
`require("shared.fixtures")` gives the exact conftest semantics for free, at both grains:

- **Within a suite** (one state, many files) — the first requirer runs the module; the rest get the
  cache. One handle, one instance, shared across the suite's files.
- **Across suites** (separate states) — each suite's cache is its own, so the module runs *fresh per
  suite*: each registers its own definitions and builds its own instances. Isolation, for free.

That is shared-definitions / per-suite-instances — the whole point of a conftest — achieved by the
language, with no auto-loaded file, no phase-gating, no "which pass am I in" flag. It also beats a
magic file on prova's own terms: **typed handles** flow through (`local F = require("fixtures");
t:use(F.pg)` — LSP works, unlike a name-string lookup), it's **traceable** (grep the requires), and
users **organize freely**. This was the original `api.md` design before the conftest detour.

### 2. `preload` is manifest-triggered `require()` — the push for what pull can't reach

`require()` is pull-based; a fixture nobody names never runs. So genuinely *ambient/autouse* global
setup (seed a temp HOME for every suite, start an ambient mock) needs a push. `preload` is that — and
it is **the same mechanism**, not a second one: prova `require()`s each entry into every suite VM
before its test files load. Entries are **module paths, identical to a `require()` arg** (`"shared.global"`,
not a file path) — one reference form everywhere. Explicit-in-the-manifest, never magic-by-filename,
because ambient-global is rare and should never be summoned by a name.

### 3. The manifest is the substrate; conventions are its defaults

Total explicitness when you want it (a template, a monorepo); zero ceremony when you don't (a service
with three test files). The two are the same model at two amounts of ceremony:

- `config` **defaults to `prova.lua`** beside the manifest; `config = "shared/config.lua"` overrides.
  The convention is just the default *value* of a manifest key.
- `preload` is **explicit-only** — no magic `global.lua` name.
- Suites: `suite.lua` convention **or** `[suites.*]` — already the model (`suites.md`'s three layers).

We deliberately do **not** go fully-explicit-no-conventions (mandatory `config`, every suite declared):
it taxes the small case for a consistency the defaults already provide. `prova.lua` stays a discovered
default, not required magic.

### 4. `suite.lua` groups a directory; it does not inherit

The one rule that kills the "why doesn't my suite fixture reach the subdir?" confusion:

- A directory with `suite.lua` → **one suite = `suite.lua` (setup) + the `*_test.lua` in that
  directory only.** Not recursive.
- Subdirectories are discovered **independently** — a child dir is its own suite (or its own
  singletons), never absorbed by a parent.
- Sharing across directories is `require()` (pull) or `preload` (push) — never silent inheritance.

Grouping is *structural* (like `package.json` marking a package); sharing is `require()`. Two clean,
separate jobs. This replaces `suites.md`'s "recursive until a nested suite.lua" (which the code never
even implemented — today an outer `suite.lua` silently swallows the whole subtree).

### 5. The constraint that governs all of it: definitions, not instances

A suite is one isolated Lua state; `Scope.Suite` fixtures are live `!Send` values. So everything
shared — via `require`, `preload`, `global.lua` — shares a **recipe**, and each suite builds its own
**instance**. A `pg` fixture required everywhere means *every suite gets its own Postgres*, never one
global container (`!Send` forbids it, and it wouldn't parallelize anyway). One genuinely-shared live
resource is an **external** real thing each suite connects to, not a prova fixture. Teach this loudly
— the naive expectation ("I put it in the shared file, so there's one") is exactly wrong.

### Bonus: `runtime.*` mutation stays out of tests

Config is a precondition, resolved once, before suites. A test mutating it makes skips
order-dependent (action-at-a-distance), misses `must_run` (checked pre-suite), and races across
parallel suite states. Tests declare *scoped* needs (`requires`, `resources`, fixtures); the
companion *computes* the runtime. Neither mutates it globally at test time.

## The gap — the engine supports none of the wiring yet

The `prova/` skeleton is the target; the engine has to be built to meet it:

1. **`config` manifest key** — companion path from the manifest, defaulting to `<home>/prova.lua`
   (today hard-coded at `main.rs:1509`). Small.
2. **Home-rooted `require()`** — put `<home>/?.lua` + `<home>/?/init.lua` on `package.path` (or a
   local searcher) so `require("shared.fixtures")` resolves `<home>/shared/fixtures.lua`. Today only
   the plugin searcher exists; local project modules are unfindable. Small, and the keystone — it is
   what makes `require`, `preload`, and the `shared/` namespace all work off one resolution rule.
3. **`preload` manifest key** — auto-`require()` its entries into each suite VM before its test files.
   Depends on #2. Medium.
4. **Directory-scoped suite discovery** — change `collect_suites` from "grab the subtree" to "this
   dir's files + recurse subdirs independently" (`suite.rs:128`). Independent of the others. Small.

## Build sequence (PDD, proved against the real `prova/` layout)

- **A. Home-rooted `require()`** — the keystone. Proof: a test `require()`s a sibling `shared/` module
  and uses a handle it returns; a second suite gets its own instance. Fill `shared/` + a
  `suites/mocks/` test to prove it.
- **B. Directory-scoped discovery** — independent, ship alongside A. Proof: `suites/a/suite.lua` and
  `suites/a/b/suite.lua` are two suites, not one; the outer does not swallow the inner's files.
- **C. `config` key** — companion from the manifest. Proof: `config = "shared/config.lua"` registers a
  capability; a bare `prova.lua` still works (the default).
- **D. `preload` key** — auto-require. Proof: an ambient fixture in a `preload`d module runs in a suite
  that never names it; and it is per-suite (two suites, two instances).

Each lands filling the `prova/` placeholders, so the repo's own suite becomes the dogfooded reference
layout — and the seed of the default `prova init` template.

## Non-goals / deferred

- **A magic global conftest file** (`global.lua`/`fixtures.lua` auto-loaded by name) — rejected;
  `require` + `preload` cover it explicitly.
- **Nested-suite fixture inheritance** — rejected; `suite.lua` is directory-local, sharing is
  `require`.
- **Tests mutating `runtime.*`** — rejected (see Bonus).
- **Per-module independent homes / monorepo glob `paths`** — a real future shape (a sub-module with
  its own `prova.toml`, or `paths = ["**/prova/suites"]` from one home), but deferred until a real
  multi-module project needs it. One home + `paths` today.
- **`prova init --template <archetype>`** — the flexibility endgame (default templates registered,
  user ones in `~/.config/prova/config.toml`, rendered via the `archetect` plugin). This canonical
  layout is what the default template *is*; build the template machinery once the layout has proven
  itself by use.
