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
`rabbitmq.container`, a package's `acme.login(ctx)`, an opinionated `service(ctx, {archetype=...})`.
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

**The only universal rule: a plugin is a Lua module that `return`s a namespace table.** Everything
below — facets, the trio, Docker, `ctx:manage` — is the convention for **one shape**, the *resource*
plugin (a provisioned or attachable server/client pair). Other shapes are equally valid and need none
of it: a **library** plugin just returns a table of helpers (custom matchers, data builders, a token
DSL); a **client-only** plugin returns a factory that attaches to an external service. Only the
resource shape touches Docker. See [ecosystem.md § Plugin shapes](ecosystem.md) for the full
taxonomy. The rest of this section describes the resource shape, since it carries the conventions
worth standardizing.

A **resource** plugin returns a namespace obeying [the namespacing grammar](namespacing.md):

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
7. **Ships a LuaCATS stub** `library/<name>.lua` (a `---@meta <name>` file) so a consumer's
   `require("<name>")` completes and type-checks in the editor. Prova syncs it automatically; the
   plugin archetype generates it. See [ide-and-layout.md](ide-and-layout.md).

A plugin author who follows this gets the same shape, IDE completion, and skip behavior as
`postgres` — because there is no difference.

## Resolution (the searcher)

`require` is wired through a custom entry appended to `package.searchers` (installed in
`plugins::install`, after the modules exist). It resolves a module name in this order:

1. **Bundled** — first-party modules embedded in the binary (`BUNDLED` registry). Reserved for the
   `prova.*` namespace. This is where migrated recipes live (see Dogfooding).
2. **Manifest-declared** — a plugin named in `prova.toml [plugins]`, resolved to an exact file (a git
   source is fetched into the cache beforehand and lands here as a path). Authoritative and pinned,
   so it wins over the disk roots below.
3. **Intra-plugin** — `<canonical>.<sub>` resolves under the plugin's own root, so a multi-file
   plugin can require its siblings without colliding with anything else.
4. **The declared plugin root**, tried as `<root>/<name-with-dots-as-slashes>.lua` then
   `.../init.lua`. It comes from the manifest's `[run] plugin_root`, resolved against the package
   root. There is no default, no environment input, and exactly one — see below.

Appended (not prepended) so it never shadows Lua's own searchers. A miss returns a message listing
where it looked, so `require`'s aggregate error is actionable. **No network fetch happens in the
searcher** — resolution is always from bundled code or a local file, which is the safety boundary
(below).

### Everything is declared

Discovery is the only implicit step: prova walks up for `.prova.toml`, `prova.toml`,
`prova/prova.toml`, or `.prova/prova.toml`. **From there the manifest names everything.**

```toml
[run]
paths        = ["proofs"]            # root-relative
config       = "config.lua"          # home-relative
plugin_root  = ".prova/plugins"      # root-relative; no default; exactly one
```

Removed, deliberately, in service of that: the per-user `data_dir/plugins` root, the
`PROVA_PLUGIN_PATH` env var, the cwd-relative `./.prova/plugins` fallback, and the engine's own
hardcoded `<project_root>/.prova/plugins` join. Each was an answer to "where could this `require`
have come from?" that you could not obtain by reading the package.

Two reasons this is worth the one line of ceremony:

- **Reproducibility.** A resolution path outside version control lets a proof pass on a laptop and
  fail in CI with nothing in the repo to explain the difference — "works on my machine", inside the
  tool whose job is to rule it out.
- **Auditability.** One file answers the question completely. That matters most when the reader is an
  agent, which cannot simply *know* a convention baked into the binary.

**One root, not a list.** The ambient root does one job — "this package's own plugins, without
naming each one" — which is inherently one place. Everything else (a vendored plugin, one from a
sibling package, a team's shared plugin) belongs in `[plugins]` with a name and a pinned path or git
source: more explicit, more reproducible, and it keeps a second directory from raising a precedence
question ("both hold `foo` — which wins?") that buys no capability.

A package declaring no root resolves no ambient plugins, and the miss message says exactly that
(`no plugin root declared — add plugin_root to [run]…`) rather than reading like a typo. The
git-checkout cache (`cache_dir/plugins`) is not an exception to any of this: its contents are pinned
by the manifest and reproducible from it.

**Testing.** Isolation comes from pointing at a manifest, not from environment injection: `--manifest`
selects the package, `--config` / `PROVA_CONFIG` selects the companion, and in-process embedders call
`RunConfig::with_plugin_root` directly. For the user-level layer, `XdgSystemLayout` honors `XDG_*` and
`RootedSystemLayout` roots every directory under one path.

**The user-level config** (`~/.config/prova/config.toml`, not yet implemented) must stay on the right
side of this line: it may change **how prova presents things** (format, jobs, colour, IDE prefs); it
may never change **what prova resolves** (plugin roots, paths, plugin sources). A user config that
could contribute a plugin root would be the machine-global plugin dir again under another name.

### Private dependencies (bundled + isolated)

The steps above are the *consumer's* namespace: anything at the top of a plugin root is ambient —
requirable by test suites and by other plugins alike, with nothing declared. A plugin may also
declare its own dependencies in its `prova.toml` (`[plugins]`):

```toml
[plugins]
inner = { path = "deps/inner" }
```

Those names resolve **for that plugin's code and nobody else's**, which is what lets a library (or a
topology) depend on something without pushing it into its consumers' namespace. The scoping happens
at *load*, by binding the chunk's environment — not in the searcher, which only ever receives a
module name and could never tell who was asking; that placement is also why a dependency required
lazily, inside a function at test time, still resolves privately. Private modules cache by path in a
registry-side table rather than in `package.loaded`, which is keyed by name and would otherwise hand
every consumer a reference.

Consequence worth knowing: a private dependency must live *inside* its dependant, not at the top of
`.prova/plugins/` — a top-level directory there is a package plugin and is globally requirable by
design. And since package plugins are ambient to each other, a plugin that requires one without
declaring it will break when lifted out to its own repo. That is an accepted trade: one rule instead
of two, and the breakage is caught by tests at extraction time.

# Topologies (advertise, register, `up`)

A topology is a whole environment addressable by name — the same definition tests use, stood up by
`prova up <name>`. Underneath it's a `prova.topology(name, fn)` registration; a plugin and a package
each get a manifest surface over that:

- A plugin **advertises** topologies in its `[plugin]` section — its public contract:

  ```toml
  [[plugin.topologies]]
  name    = "linux-vm"
  factory = "topologies.linux_vm"   # a dotted path into the plugin's returned namespace
  ```

- A package **registers** which to expose, in `[topologies]` — by advertised name (the encapsulated
  form) or by a direct factory path (for your own plugins, where there's no contract to mediate):

  ```toml
  [topologies]
  vm  = { plugin = "parallels", topology = "linux-vm" }   # via the advertisement
  dev = { plugin = "lib",       factory  = "topologies.dev" }
  ```

Each entry desugars to `prova.topology("<name>", require("<plugin>").<factory>)`, execed after the
definition files, so a manifest topology is indistinguishable from a Lua-declared one. `prova up`
lists them; `prova up <name>` stands one up. The synthesized source is validated (name and dotted
identifier paths) before splicing, so a manifest can never inject Lua; a reference to a factory or an
advertised name that doesn't exist fails loudly, naming what *is* available.

**A topology declares the environment it needs** — `requires` on the advertisement (the topology's
own contract) and/or the registration (a local addition), merged:

```toml
[[plugin.topologies]]
name     = "linux-vm"
factory  = "topologies.linux_vm"
requires = ["parallels"]          # this topology needs the Parallels VM host
```

`prova up <name>` checks these against the same capability set `requires` uses, *before* provisioning:
an unmet requirement stops it early with a clear reason (`cannot stand up "vm": it requires
"parallels" is unavailable`) instead of failing deep in a factory. The requirement travels with the
topology, so it holds for every package that registers it — the environment gate propagates even
though the factory's implementation stays the plugin's own business.

Because a plugin is a package with its own suite (§ one manifest), a plugin that advertises a topology
can prove it in its own `proofs/` — so every advertised topology ships with the suite that verifies it.

**From a git repo, no local package needed.** The same advertisement drives the remote forms of `up`:
`prova up <url>` fetches a repo (pinned + freshness-gated, like a git `[plugins]` source) and lists
the topologies it advertises; `prova up <topology> <url>` stands one up directly. The repo is resolved
as a plugin under an internal require-name, its advertised factory is registered as that topology, and
the advertised `requires` gate the stand-up — so `prova up linux-vm github.com/acme/prova-parallels`
grabs a proven topology from anywhere.

Wired now (the "easy to install" story):

- **XDG layout** (`layout.rs`, `SystemLayout`) — `config_dir` `~/.config/prova`, `cache_dir`
  `~/.cache/prova`, `data_dir` `~/.local/share/prova` (XDG on macOS too, like archetect;
  `XDG_*` honored). `XdgSystemLayout` for production, `RootedSystemLayout` for tests.
- **The declared plugin root** — `[run] plugin_root` in the manifest, resolved against the package
  root. The only directory scanned; there is no global install dir (see "Everything is declared").
- **Manifest-declared plugins** — `prova.toml` `[plugins]` maps a name to a local path or a **git
  source** (`{ git = "…", tag/branch/rev = "…", module = "…" }`). Git sources are fetched (shelling
  to `git`, like archetect fetches archetype sources) into `cache_dir/plugins`, pinned by ref and
  reused on the next run. The resolved `name → file` map is authoritative over disk roots, so a
  declared plugin resolves the same way in every environment:

  ```toml
  [sources]                                                           # register org aliases
  acme = "github:acme"

  [plugins]
  greet    = "./plugins/greet.lua"                                    # local path
  redis    = "acme:prova-redis@v1"                                    # alias shorthand → github.com/acme/prova-redis
  loadtest = "github:acme/prova-loadtest@v2"                          # host shorthand
  vault    = "acme/prova-vault@v3"                                    # bare org/repo (defaults to github)
  rabbitmq = { git = "https://github.com/acme/prova-rabbitmq", tag = "v1.0.0" }
  nats     = { git = "https://github.com/acme/prova-nats", rev = "abc123", module = "src/nats.lua" }
  ```

  A bare `org/repo` shorthand **requires an `@ref`** so a plain relative path is never mistaken for a
  remote (a surprise fetch); use `github:org/repo` for a ref-less remote, or the table form for a
  commit `rev`. `@ref` maps to `git clone --branch`, which accepts a tag *or* a branch.

- **Plugin section** (`prova.toml [plugin]`) — a published plugin carries its contract in the SAME
  `prova.toml` a package uses (there is no separate file); the `[plugin]` table is the analogue of
  archetect's `archetype.yaml`, and a repo with `[plugin]` + `[run]` is a package that is both a
  plugin and its own suite:

  ```toml
  [plugin]
  name  = "rabbitmq"        # canonical namespace (for intra-plugin require); defaults to the key
  entry = "rabbitmq.lua"    # the entry file — resolution no longer depends on the consumer's alias
  description = "…"
  license = "MIT"

  [requires]
  prova = ">=0.1, <0.2"     # compatibility range — refuses to load outside it (semver VersionReq)
  ```

  - **`entry`** removes the frail step: the author declares the entry file once, so a consumer can
    pull the plugin under *any* alias (`mq = "prova-rs/prova-rabbitmq@v1"`) and it still resolves.
    Entry precedence for a directory source: consumer `module =` override → manifest `entry` →
    `init.lua` → `<alias>.lua` (last-ditch back-compat; the reason to declare `entry`).
  - **`[requires] prova`** gates compatibility against the running version, exactly like
    `requires.archetect` — a clear error, not a mysterious runtime failure, when a plugin is too new
    or too old. On 0.x the minor is the breaking axis (`^0.1` = `>=0.1.0, <0.2.0`).
  - **Intra-plugin `require`.** A multi-file plugin requires its own siblings by its **canonical**
    name — `require("rabbitmq.helpers")` → `<plugin-root>/helpers.lua` — namespaced so it is stable
    regardless of the consumer's alias and never collides with another plugin. This is the sanctioned
    way to split a plugin into files (see the self-contained rule in
    [ecosystem.md](ecosystem.md)). Plugins vendor their **helpers** this way (intra-plugin requires,
    by canonical name); **inter-plugin** dependencies — a library plugin that reuses `postgres` — are
    declared in `[dependencies]` and resolved privately per plugin, invisible to the consumer. See
    [plugin-composition.md](plugin-composition.md).

Not yet wired, deliberately deferred:

- **`prova.use(name)`** sugar — `require` + install as a global namespace, for plugins that want
  first-party-style ergonomics.
- **A `prova plugin add …` subcommand** — resolve + install into `data_dir/plugins` from the CLI
  (today: edit `[plugins]` or drop a file).
- **Global `~/.config/prova` config** — the layout exposes `config_dir`; nothing reads it yet.

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

- **Done:** custom searcher (bundled → manifest-named → intra-plugin → disk roots); one bundled
  loadable namespace (`prova.workspace`); ambient plugins via the declared `[run] plugin_root`;
  the XDG `SystemLayout`; `[plugins]` manifest sources with **git fetch + cache**,
  verified end-to-end through the real binary (`tests/plugin_git.rs`); **private plugin dependencies**
  (`prova.toml [plugins]`), scoped at load via the chunk environment and cached by path
  (`tests/plugin_private_deps.rs`, `proofs/plugins/`). Existing globals unchanged and first-class.
- **Removed:** the machine-global `data_dir/plugins` root — nothing populated it, and it was a
  "works on my machine" path outside version control (see above).
- **Next:** migrate a first real recipe (e.g. `redis`) to the bundled loadable path with a parity
  test (dogfooding); `prova.use`; a `prova plugin add` subcommand; read `~/.config/prova`.
- **Later:** the sidecar protocol for native Tier-2 plugins, if a real need appears.
