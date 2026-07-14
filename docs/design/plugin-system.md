# Plugin System

Drafted 2026-07-13. Records how Prova is extended — by users and by us — and the seam that makes
both the same. Builds directly on [namespacing.md](namespacing.md) (a plugin *is* a namespace) and
the recipe pattern already in `modules.rs`.

## The insight this is built on

Prova already has two layers, and the top one is pure Lua:

- **Primitives (Rust).** Thin bindings that *must* be native because they wrap a client or do
  something blocking/unsafe: `postgres.client(url)` (sqlx), `docker.run(...)` (bollard), `http.get`,
  `net.free_port`, `prova.retry`, `ctx:manage`. Registered as globals in `modules.rs::install`.
- **Recipes (Lua).** Sugar that *composes* primitives. `postgres.container(ctx, opts)` is a Lua
  chunk (`POSTGRES_RECIPES_LUA`) `include_str!`'d into the binary and `lua.load(...).exec()`'d at
  startup. Its whole body is `docker.run` + `postgres.client` + `prova.retry` + `ctx:manage` —
  nothing a user couldn't type.

The recipes have **no privileged access**. The only thing making them "first-party" is cosmetic:
they are baked into the binary and injected as globals instead of loaded from disk. A plugin system
is therefore not new machinery bolted on — it is **handing users the same authoring seam the recipes
already use**, plus a resolver so `require` can find their code.

## Two tiers, deliberately unequal

**Tier 1 — Lua plugins (the 95% case).** New recipes composing existing primitives: a
`rabbitmq.container`, a project's `acme.login(ctx)`, an opinionated `service(ctx, {archetype=...})`.
Pure Lua, no compile, distributable as a file or a git repo. This is the tier we invest in.

**Tier 2 — Native plugins (rare, genuinely hard).** A *new primitive* — e.g. a native NATS client —
needs a Rust crate linked in. You cannot dynamically load that into one static binary cleanly.
Realistic options, best-fit first:

1. **Cargo feature + "build your own distribution."** Legitimate for a Rust binary; the primitive
   set stays curated and we cut releases with the batteries we choose. This is the status quo and
   the recommended path for native extension.
2. **Out-of-process sidecar.** A plugin is a subprocess speaking a small protocol over stdio. We
   already have `shell.spawn` and a JSONL event bus, so this is a natural (future) extension.
   ABI-safe and language-agnostic; slower.
3. **cdylib / C-ABI FFI.** Avoid. mlua across a dynamic boundary is a maintenance sinkhole.

**Decision: keep the native primitive set broad and curated in-tree; make Tier 1 first-class.** We
do not build dynamic native loading. "New primitive" = a PR to prova or a custom build, not a
plugin.

## The contract (this *is* the plugin API)

A plugin is a Lua module that `return`s a **namespace table** obeying
[the namespacing grammar](namespacing.md):

```lua
-- rabbitmq.lua — a third-party plugin, one namespace, standard facets.
local rabbitmq = {}

function rabbitmq.client(url) ... end                 -- attach to something running

function rabbitmq.container(ctx, opts)                -- provision + wait + manage teardown
  opts = opts or {}
  local c = ctx:manage(docker.run{ image = opts.image or "rabbitmq:3", ports = { 5672 },
                                   wait = { port = 5672, timeout = opts.timeout or "60s" } })
  local url = "amqp://127.0.0.1:" .. c:host_port(5672)
  local client = ctx:manage(prova.retry(function() return rabbitmq.client(url) end,
                            { timeout = opts.timeout or "60s" }))
  return { client = client, url = url, container = c }   -- the guaranteed trio
end

return rabbitmq
```

The contract is exactly the conventions the first-party recipes already follow:

1. **Namespace = the API you speak.** One table, technology-first name.
2. **`(ctx, opts)`, context first.** Any recipe that owns a resource takes the fixture/test context
   as its first argument.
3. **Lifecycle through `ctx:manage` / `ctx:defer`.** Never leak; teardown ties to the scope. A
   managed value just needs a `stop()` or `close()` method.
4. **Readiness through `prova.retry`.** Don't sleep; retry the real thing.
5. **`container` returns the trio** `{ client, url, container }` (extras allowed, trio guaranteed).
6. **`requires` for graceful skip.** A recipe touching Docker lets its tests declare
   `requires = { "docker" }`; the existing skip-fixpoint handles absence for free.

A plugin author who follows this gets the same shape, IDE completion, and skip behavior as
`postgres` — because there is no difference.

## Resolution (the searcher)

`require` is wired through a custom entry appended to `package.searchers` (installed in
`plugins::install`, after the modules exist). It resolves a module name in this order:

1. **Bundled** — first-party modules embedded in the binary (`BUNDLED` registry). Reserved for the
   `prova.*` namespace. This is where migrated recipes live (see Dogfooding).
2. **Disk roots**, each tried as `<root>/<name-with-dots-as-slashes>.lua` then `.../init.lua`:
   - every dir on `PROVA_PLUGIN_PATH` (colon-separated), then
   - `./.prova/plugins/` (project-local).

Appended (not prepended) so it never shadows Lua's own searchers. A miss returns a message listing
where it looked, so `require`'s aggregate error is actionable. **No network fetch happens in the
searcher** — resolution is always from bundled code or a local file, which is the safety boundary
(below).

Not yet wired, deliberately deferred:

- **Global install dir** (XDG `~/.local/share/prova/plugins/`) — needs a path-layout decision; today
  use `PROVA_PLUGIN_PATH`.
- **Manifest-declared plugins** — `prova.toml` listing plugin sources (local paths or **git URLs
  fetched + cached like archetect sources**), optionally auto-installed as globals so a suite's
  plugins are declared once. This is the real "easy to install" story and the next step after the
  searcher proves out.
- **`prova.use(name)`** sugar — `require` + install as a global namespace, for plugins that want
  first-party-style ergonomics.

## Safety

Plugins are Lua running in the **same context as the tests** — they already have `shell`, `fs`,
`docker`, and network primitives. There is no runtime sandbox between a plugin and a test, and
adding one would gut the point (a test framework must drive real systems). So "safe to install" is
about **provenance, not confinement**:

- The searcher only loads **bundled code or explicit local files** — never an implicit download.
  Getting a plugin onto disk is a deliberate act (copy a file, or later, a manifest entry you can
  read in review).
- When manifest git-fetch lands, it inherits archetect's model: pinned refs, a local cache, and the
  source URL visible in `prova.toml` and in review — the same trust posture as depending on any git
  crate.
- A plugin is code you run. We treat installing one exactly like adding a dependency: you vet the
  source. The framework's job is to make the source **explicit and pinned**, not to pretend
  untrusted plugins can be run safely.

## Dogfooding

Once the searcher exists, the first-party recipes should **migrate out of `include_str!` and into
bundled Lua modules loaded through the same searcher.** If our recipes go through the user's front
door, the door works — the same lesson as the archetype starters. `postgres.container` becoming just
another resolvable module is the acceptance test for the whole system.

We keep the migration gradual: some namespaces stay first-class globals (installed eagerly) while
the loadable path matures. A recipe is a candidate to move once it can be `require`d, IDE-annotated,
and tested through the public seam with no behavior change.

## Status

- **Now (spike):** custom searcher installed; bundled + `PROVA_PLUGIN_PATH` + `./.prova/plugins`
  resolution; one bundled loadable namespace (`prova.workspace`) and a disk-plugin example proving
  user authorship. Existing globals unchanged and first-class.
- **Next:** manifest-declared plugin sources with git fetch/cache; `prova.use`; global XDG dir;
  migrate a first real recipe (e.g. `redis`) to the bundled loadable path with a parity test.
- **Later:** the sidecar protocol for native Tier-2 plugins, if a real need appears.
