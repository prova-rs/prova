# Package System

Drafted 2026-07-13. Records how Prova is extended — by users and by us — and the seam that makes
both the same. Builds directly on [namespacing.md](namespacing.md) (a package *is* a namespace) and
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
they are baked into the binary and injected as globals instead of loaded from disk. A package system
is therefore not new machinery bolted on — it is **handing users the same authoring seam the recipes
already use**, plus a resolver so `require` can find their code.

## Two tiers, deliberately unequal

**Tier 1 — Lua packages (the 95% case).** New recipes composing existing primitives: a
`rabbitmq.container`, a package's `acme.login(ctx)`, an opinionated `service(ctx, {archetype=...})`.
Pure Lua, no compile, distributable as a file or a git repo. This is the tier we invest in.

**Tier 2 — Native packages (rare, genuinely hard).** A *new primitive* — e.g. a native NATS client —
needs a Rust crate linked in. You cannot dynamically load that into one static binary cleanly.
Realistic options, best-fit first:

1. **Cargo feature + "build your own distribution."** Legitimate for a Rust binary; the primitive
   set stays curated and we cut releases with the batteries we choose. This is the status quo and
   the recommended path for native extension.
2. **Out-of-process sidecar.** A package is a subprocess speaking a small protocol over stdio. We
   already have `shell.spawn` and a JSONL event bus, so this is a natural (future) extension.
   ABI-safe and language-agnostic; slower.
3. **cdylib / C-ABI FFI.** Avoid. mlua across a dynamic boundary is a maintenance sinkhole.

**Decision: keep the native primitive set broad and curated in-tree; make Tier 1 first-class.** We
do not build dynamic native loading. "New primitive" = a PR to prova or a custom build, not a
package.

## The contract (this *is* the package API)

**The only universal rule: a package is a Lua module that `return`s a namespace table.** Everything
below — facets, the trio, Docker, `ctx:manage` — is the convention for **one shape**, the *resource*
package (a provisioned or attachable server/client pair). Other shapes are equally valid and need none
of it: a **library** package just returns a table of helpers (custom matchers, data builders, a token
DSL); a **client-only** package returns a factory that attaches to an external service. Only the
resource shape touches Docker. See [ecosystem.md § Package shapes](ecosystem.md) for the full
taxonomy. The rest of this section describes the resource shape, since it carries the conventions
worth standardizing.

A **resource** package returns a namespace obeying [the namespacing grammar](namespacing.md):

```lua
-- rabbitmq.lua — a third-party package, one namespace, standard facets.
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
   package archetype generates it. See [ide-and-layout.md](ide-and-layout.md).

A package author who follows this gets the same shape, IDE completion, and skip behavior as
`postgres` — because there is no difference.

<!-- claim: pkg-dir-locates-the-package -->
One binding the runtime adds for free: every package chunk runs with a per-package **`pkg`** table,
and `pkg.dir` is the directory the package's own file lives in. That is the anchor for locating the
package's *own* artifacts — `prova.root` is the **consuming** package's root, so a package reused
cross-repo would resolve the consumer's `target/`, not its own. (`pkg`, not `package`: that name is
Lua's own module table. `plugin` is the deprecated alias, retiring at 1.0.)

## Resolution (the searcher)

<!-- claim: resolution-order -->
`require` is wired through a custom entry appended to `package.searchers` (installed in
`packages::install`, after the modules exist). It resolves a module name in this order:

1. **Bundled** — first-party modules embedded in the binary (`BUNDLED` registry). Reserved for the
   `prova.*` namespace. This is where migrated recipes live (see Dogfooding).
2. **Manifest-declared** — a package named in `prova.toml [dependencies]`, resolved to an exact file (a git
   source is fetched into the cache beforehand and lands here as a path). Authoritative and pinned,
   so it wins over the disk roots below.
3. **Intra-package** — `<canonical>.<sub>` resolves under the package's own root, so a multi-file
   package can require its siblings without colliding with anything else.
4. **The declared package root**, tried as `<root>/<name-with-dots-as-slashes>.lua` then
   `.../init.lua`. It comes from the manifest's `[run] packages`, resolved against the package
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
proofs       = ["proofs"]            # directory-name patterns; default, usually omitted — match every such dir anywhere below the root
config       = "config.lua"          # home-relative
packages  = ".prova/packages"      # root-relative; no default; exactly one
```

<!-- claim: declared-root-only -->
Removed, deliberately, in service of that: the per-user `data_dir/packages` root, the
`PROVA_PLUGIN_PATH` env var, the cwd-relative `./.prova/packages` fallback, and the engine's own
hardcoded `<project_root>/.prova/packages` join. Each was an answer to "where could this `require`
have come from?" that you could not obtain by reading the package.

Two reasons this is worth the one line of ceremony:

- **Reproducibility.** A resolution path outside version control lets a proof pass on a laptop and
  fail in CI with nothing in the repo to explain the difference — "works on my machine", inside the
  tool whose job is to rule it out.
- **Auditability.** One file answers the question completely. That matters most when the reader is an
  agent, which cannot simply *know* a convention baked into the binary.

**One root, not a list.** The ambient root does one job — "this package's own packages, without
naming each one" — which is inherently one place. Everything else (a vendored package, one from a
sibling package, a team's shared package) belongs in `[dependencies]` with a name and a pinned path or git
source: more explicit, more reproducible, and it keeps a second directory from raising a precedence
question ("both hold `foo` — which wins?") that buys no capability.

<!-- claim: undeclared-root-teaches -->
A package declaring no root resolves no ambient packages, and the miss message says exactly that
(`no package root declared — add packages to [run] in prova.toml`) rather than reading like a
typo — and the key it teaches is one the manifest actually accepts. The git-checkout cache
(`cache_dir/packages`) is not an exception to any of this: its contents are pinned by the manifest
and reproducible from it.

**Testing.** Isolation comes from pointing at a manifest, not from environment injection: `--manifest`
selects the package, `--config` / `PROVA_CONFIG` selects the companion, and in-process embedders call
`RunConfig::with_package_root` directly. For the user-level layer, `XdgSystemLayout` honors `XDG_*` and
`RootedSystemLayout` roots every directory under one path.

**The user-level config** (`~/.config/prova/config.toml`, not yet implemented) must stay on the right
side of this line: it may change **how prova presents things** (format, jobs, colour, IDE prefs); it
may never change **what prova resolves** (package roots, proofs, package sources). A user config that
could contribute a package root would be the machine-global package dir again under another name.

### Private dependencies (bundled + isolated)

The steps above are the *consumer's* namespace: anything at the top of a package root is ambient —
requirable by test suites and by other packages alike, with nothing declared. A package may also
declare its own dependencies in its `prova.toml` (`[dependencies]`):

```toml
[dependencies]
inner = { path = "deps/inner" }
```

<!-- claim: private-dependencies-isolated -->
For an **ambient** package (one living under the declared root), those names resolve **for that
package's code and nobody else's**, which is what lets a library (or a topology) depend on something
without pushing it into its consumers' namespace. The scoping happens at *load*, by binding the
chunk's environment — not in the searcher, which only ever receives a module name and could never
tell who was asking; that placement is also why a dependency required lazily, inside a function at
test time, still resolves privately. Private modules cache by path in a registry-side table rather
than in `package.loaded`, which is keyed by name and would otherwise hand every consumer a
reference.

<!-- claim: transitive-dependencies-resolve -->
A **declared** package's dependencies are *followed* rather than scoped: the resolver walks its
`[dependencies]` (and theirs) breadth-first, so a consumer that pins one composing package gets
everything it needs without re-declaring internals it never mentions. Three rules make that safe: a
dependency's relative `path` resolves against the package that **declared** it, never the consumer;
an **explicit declaration always beats a transitive one** (a package owns its own environment — a
dependency cannot swap a version out from under it); and a dependency cycle terminates instead of
looping. Today the followed names land in the consumer's namespace — the peer mode of
[package-composition.md](package-composition.md); scoping *declared* dependencies per package the
way ambient ones already are is the engine change that doc still owes.

Consequence worth knowing: a private dependency must live *inside* its dependant, not at the top of
`.prova/packages/` — a top-level directory there is an ambient package and is globally requirable by
design. And since ambient packages can require each other freely, a package that requires one
without declaring it will break when lifted out to its own repo. That is an accepted trade: one rule
instead of two, and the breakage is caught by tests at extraction time.

# Topologies (advertise, register, `up`)

A topology is a whole environment addressable by name — the same definition tests use, stood up by
`prova up <name>`. Underneath it's a `prova.topology(name, fn)` registration; the providing package
and the consuming package each get a manifest surface over that:

- A package **advertises** topologies in its `[package]` section — its public contract:

  ```toml
  [[package.topologies]]
  name    = "linux-vm"
  factory = "topologies.linux_vm"   # a dotted path into the package's returned namespace
  ```

- A package **registers** which to expose, in `[topologies]` — by advertised name (the encapsulated
  form) or by a direct factory path (for your own packages, where there's no contract to mediate):

  ```toml
  [topologies]
  vm  = { package = "parallels", topology = "linux-vm" }   # via the advertisement
  dev = { package = "lib",       factory  = "topologies.dev" }
  ```

<!-- claim: topology-advertisement-resolves -->
Each entry desugars to `prova.topology("<name>", require("<package>").<factory>)`, execed after the
definition files, so a manifest topology is indistinguishable from a Lua-declared one. `prova up`
lists them; `prova up <name>` stands one up. The synthesized source is validated (name and dotted
identifier paths) before splicing, so a manifest can never inject Lua; a reference to a factory or an
advertised name that doesn't exist fails loudly, naming what *is* available.

**A topology declares the environment it needs** — `requires` on the advertisement (the topology's
own contract) and/or the registration (a local addition), merged:

```toml
[[package.topologies]]
name     = "linux-vm"
factory  = "topologies.linux_vm"
requires = ["parallels"]          # this topology needs the Parallels VM host
```

<!-- claim: up-gates-on-requires -->
`prova up <name>` checks these against the same capability set `requires` uses, *before* provisioning:
an unmet requirement stops it early with a clear reason (`cannot stand up topology "vm": it requires
"parallels" is unavailable`) instead of failing deep in a factory. The requirement travels with the
topology, so it holds for every package that registers it — the environment gate propagates even
though the factory's implementation stays the package's own business.

Because a package carries its own suite (§ one manifest), a package that advertises a topology can
prove it in its own `proofs/` — so every advertised topology ships with the suite that verifies it.

**From a git repo, no local package needed.** The same advertisement drives the remote forms of `up`:
`prova up <url>` fetches a repo (pinned + freshness-gated, like a git `[dependencies]` source) and lists
the topologies it advertises; `prova up <topology> <url>` stands one up directly. The repo is resolved
as a package under an internal require-name, its advertised factory is registered as that topology, and
the advertised `requires` gate the stand-up — so `prova up linux-vm github.com/acme/prova-parallels`
grabs a proven topology from anywhere.

Wired now (the "easy to install" story):

- **XDG layout** (`layout.rs`, `SystemLayout`) — `config_dir` `~/.config/prova`, `cache_dir`
  `~/.cache/prova`, `data_dir` `~/.local/share/prova` (XDG on macOS too, like archetect;
  `XDG_*` honored). `XdgSystemLayout` for production, `RootedSystemLayout` for tests.
- **The declared package root** — `[run] packages` in the manifest, resolved against the package
  root. The only directory scanned; there is no global install dir (see "Everything is declared").
- **Manifest-declared packages** — `prova.toml` `[dependencies]` maps a name to a local path or a **git
  source** (`{ git = "…", tag/branch/rev = "…", module = "…" }`). Git sources are fetched (shelling
  to `git`, like archetect fetches archetype sources) into `cache_dir/packages`, pinned by ref and
  reused on the next run. The resolved `name → file` map is authoritative over disk roots, so a
  declared package resolves the same way in every environment:

  ```toml
  [sources]                                                           # register org aliases
  acme = "github:acme"

  [dependencies]
  greet    = "./packages/greet.lua"                                    # local path
  redis    = "acme:prova-redis@v1"                                    # alias shorthand → github.com/acme/prova-redis
  loadtest = "github:acme/prova-loadtest@v2"                          # host shorthand
  vault    = "acme/prova-vault@v3"                                    # bare org/repo (defaults to github)
  rabbitmq = { git = "https://github.com/acme/prova-rabbitmq", tag = "v1.0.0" }
  nats     = { git = "https://github.com/acme/prova-nats", rev = "abc123", module = "src/nats.lua" }
  ```

  A bare `org/repo` shorthand **requires an `@ref`** so a plain relative path is never mistaken for a
  remote (a surprise fetch); use `github:org/repo` for a ref-less remote, or the table form for a
  commit `rev`. `@ref` maps to `git clone --branch`, which accepts a tag *or* a branch.

- **Package section** (`prova.toml [package]`) — a published package carries its contract in the SAME
  `prova.toml` a package uses (there is no separate file); the `[package]` table is the analogue of
  archetect's `archetype.yaml`, and a repo with `[package]` + `[run]` is a package that is both a
  package and its own suite:

  ```toml
  [package]
  name  = "rabbitmq"        # canonical namespace (for intra-package require); defaults to the key
  entry = "rabbitmq.lua"    # the entry file — resolution no longer depends on the consumer's alias
  description = "…"
  license = "MIT"

  [requires]
  prova = ">=0.1, <0.2"     # compatibility range — refuses to load outside it (semver VersionReq)
  ```

  <!-- claim: entry-decouples-alias -->
  - **`entry`** removes the frail step: the author declares the entry file once, so a consumer can
    pull the package under *any* alias (`mq = "prova-rs/prova-rabbitmq@v1"`) and it still resolves.
    Entry precedence for a directory source: consumer `module =` override → manifest `entry` →
    `init.lua` → `<alias>.lua` (last-ditch back-compat; the reason to declare `entry`).
  <!-- claim: requires-prova-gates-load -->
  - **`[requires] prova`** gates compatibility against the running version, exactly like
    `requires.archetect` — a clear error, not a mysterious runtime failure, when a package is too new
    or too old. On 0.x the minor is the breaking axis (`^0.1` = `>=0.1.0, <0.2.0`).
  - **Intra-package `require`.** A multi-file package requires its own siblings by its **canonical**
    name — `require("rabbitmq.helpers")` → `<package-root>/helpers.lua` — namespaced so it is stable
    regardless of the consumer's alias and never collides with another package. This is the sanctioned
    way to split a package into files (see the self-contained rule in
    [ecosystem.md](ecosystem.md)). Packages vendor their **helpers** this way (intra-package requires,
    by canonical name); **inter-package** dependencies — a library package that reuses `postgres` — are
    declared in `[dependencies]` and resolved privately per package, invisible to the consumer. See
    [package-composition.md](package-composition.md).

Not yet wired, deliberately deferred:

- **`prova.use(name)`** sugar — `require` + install as a global namespace, for packages that want
  first-party-style ergonomics.
- **A `prova package add …` subcommand** — resolve + install into `data_dir/packages` from the CLI
  (today: edit `[dependencies]` or drop a file).
- **Global `~/.config/prova` config** — the layout exposes `config_dir`; nothing reads it yet.

## Safety

Packages are Lua running in the **same context as the tests** — they already have `shell`, `fs`,
`docker`, and network primitives. There is no runtime sandbox between a package and a test, and
adding one would gut the point (a test framework must drive real systems). So "safe to install" is
about **provenance, not confinement**:

- The searcher only loads **bundled code or explicit local files** — never an implicit download.
  Getting a package onto disk is a deliberate act (copy a file, or later, a manifest entry you can
  read in review).
- When manifest git-fetch lands, it inherits archetect's model: pinned refs, a local cache, and the
  source URL visible in `prova.toml` and in review — the same trust posture as depending on any git
  crate.
- A package is code you run. We treat installing one exactly like adding a dependency: you vet the
  source. The framework's job is to make the source **explicit and pinned**, not to pretend
  untrusted packages can be run safely.

## Dogfooding

Once the searcher exists, the first-party recipes should **migrate out of `include_str!` and into
bundled Lua modules loaded through the same searcher.** If our recipes go through the user's front
door, the door works — the same lesson as the archetype starters. `postgres.container` becoming just
another resolvable module is the acceptance test for the whole system.

We keep the migration gradual: some namespaces stay first-class globals (installed eagerly) while
the loadable path matures. A recipe is a candidate to move once it can be `require`d, IDE-annotated,
and tested through the public seam with no behavior change.

## Status

- **Done:** custom searcher (bundled → manifest-named → intra-package → disk roots); bundled
  loadable namespaces (`prova.workspace`, `prova.double`); ambient packages via the declared
  `[run] packages`;
  the XDG `SystemLayout`; `[dependencies]` manifest sources with **git fetch + cache**,
  verified end-to-end through the real binary (`tests/plugin_git.rs`); **private package dependencies**
  (`prova.toml [dependencies]`), scoped at load via the chunk environment and cached by path
  (`tests/plugin_private_deps.rs`, `proofs/packages/`). Existing globals unchanged and first-class.
- **Removed:** the machine-global `data_dir/packages` root — nothing populated it, and it was a
  "works on my machine" path outside version control (see above).
- **Next:** migrate a first real recipe (e.g. `redis`) to the bundled loadable path with a parity
  test (dogfooding); `prova.use`; a `prova package add` subcommand; read `~/.config/prova`.
- **Later:** the sidecar protocol for native Tier-2 packages, if a real need appears.
