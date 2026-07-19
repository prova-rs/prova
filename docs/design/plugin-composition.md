# Plugin Composition & Isolation

Drafted 2026-07-19. How one plugin depends on another **without** the hairiness that
[plugin-system.md](plugin-system.md) rightly forbids. This doc **amends** that document's current
stance (§ Resolution: "plugins vendor their helpers; there is no dependency resolver") — the arrival
of plugins that export **topologies, tests, and groups as libraries** (the `prova-mocks` workspace)
makes a constrained, *isolated* dependency resolver necessary. Builds on
[plugin-system.md](plugin-system.md), [namespacing.md](namespacing.md), and
[ecosystem.md](ecosystem.md).

## The problem

A plugin is no longer only a recipe. `prova-mocks` is growing the ability to export a **topology** —
a reusable, drop-in slice of a system — as a library. A realistic one is "a service with a Postgres
backing store." That topology *needs* Postgres, which lives in another plugin (`prova-postgres`).

So a plugin now genuinely wants to depend on another plugin — the exact thing
[plugin-system.md](plugin-system.md) says we don't do. The rule was right for the world it was
written in (self-contained recipes that vendor their own helpers). It is now too strong. The task is
to allow the dependency while keeping the property the rule was protecting.

## "Depend on another plugin" is three different things

The word hides three couplings, and only one is actually forbidden:

- **(a) code / ABI dependency** — plugin A links plugin B's *Rust symbols*. This is the hairy one
  (load order, version skew, ABI). This stays forbidden; it is what Tier-2 native plugins already
  refuse (see [plugin-system.md](plugin-system.md) § Two tiers).
- **(b) capability dependency** — A's topology needs *some* provider of `postgres`. It does not care
  which crate. This is what the topology case actually is.
- **(c) namespace dependency** — A's Lua calls `postgres.container(...)`, needing that module to
  resolve. Also what the topology case is.

The topology case is **(b) + (c), not (a).** Two Lua plugins composing does not link any Rust across a
boundary. So we can allow it.

## Decision: bundled + isolated, by default

A plugin declares its plugin dependencies in its own manifest, and they resolve **privately**: an
inner plugin a library pulls in is **never exposed to the library's consumer.** This is the
cargo/npm/go-modules mental model every engineer already holds — your transitive dependencies are
yours, not your caller's.

Concretely:

- `prova-shop-topologies` declares a dependency on `prova-postgres`.
- The consumer adds **only** `prova-shop-topologies` to `[plugins]`. They never see `postgres` in
  their namespace, cannot accidentally couple to it, and two libraries may even resolve *different*
  Postgres providers or versions without colliding.

This is strictly more correct than the alternative of dumping providers into a shared namespace, and —
because it matches the package-manager model — it is *less* to learn, not more: it deletes the entire
"will my `postgres` collide with the library's?" class of worry.

### What this is NOT

**Not a security sandbox.** [plugin-system.md](plugin-system.md) § Safety is explicit: plugins run in
the same context as tests, with `shell`/`fs`/`docker`, and a runtime sandbox would gut the point. That
still holds. The isolation here is **namespace-graph encapsulation** — which module a given `require`
resolves to, and who can name whom — *not* confinement of what a plugin may touch. A dependency is
still code you vet, pinned and visible in review, exactly as today.

## The mechanism (grounded in the searcher)

The isolation machinery mostly **already exists**. `plugins.rs` already does per-plugin **canonical
namespacing**: a plugin's intra-package `require("<canonical>.<sub>")` resolves against *its own*
root, alias-independent and collision-free (see [plugin-system.md](plugin-system.md) § Resolution,
"Intra-plugin `require`"). Private, scoped resolution *within* a plugin is real today.

The one gap is that today the searcher resolves a bare `require("pg")` against a **flat named map**
(the *consumer's* `[plugins]` aliases) plus disk roots, and it only receives the *name* being
required — not *which plugin's code is doing the requiring*. So the extension is:

1. **A per-plugin private dependency map.** `prova-plugin.toml` declares the plugin's own plugin
   dependencies. Resolution of a `require` originating in plugin `P` consults `P`'s dependency map
   first — so `require("pg")` inside `P` hits `P`'s declared `prova-postgres`, invisibly to the
   consumer.
2. **Binding the requiring plugin to the loader.** Because the searcher must know *who is requiring*,
   each plugin's chunk is loaded with a plugin-scoped `require` (via its `_ENV`/loader), so even a
   lazy `require` evaluated at test-time still resolves against the right plugin's map, not the
   consumer's.

This is an engine change in the collision-zone files (`prova-core` / `prova-cli` `plugins.rs`). It is
a natural extension of the canonical-namespace searcher, not a new subsystem — but it must have a
**single owner** to avoid re-opening the collision those files were just untangled from.

### Manifest surface

```toml
# prova-plugin.toml — the library that reuses Postgres
[plugin]
name  = "shop-topologies"
entry = "shop.lua"

[requires]
prova    = ">=0.1, <0.2"   # existing: compatibility with the running prova

[dependencies]              # NEW: private plugin dependencies, resolved against this plugin only
postgres = "prova-rs/prova-postgres@v1"
```

The `[dependencies]` sources reuse the *same* grammar `[plugins]` already accepts (local path, git
source, org/repo@ref) — one resolver, two entry points.

## Two correctness invariants (what makes isolation *correct*, not just tidy)

Isolating the **surface** is right, but two things must **not** be isolated, or we recreate the
vacuous green this framework exists to remove:

1. **`requires` propagates transitively even though the module does not.** The consumer never typed
   `docker`, but if the hidden `prova-postgres` declares `requires = { "docker" }`, the topology's
   tests must still **skip** on a docker-less box — with a reason that **names the chain**
   (`orders-with-store → prova-postgres → docker`). Hide the API; propagate the environment gate.
   This reuses the existing leaf-`requires` inheritance and `resolve_requires` skip-fixpoint: fold a
   dependency's manifest `requires` into the `requires` of every leaf that transitively pulls it in.
   (Today `[requires]` carries only `prova`; this extends it to capability expressions and makes them
   propagate.)

2. **Resource sharing is a separate axis — fixtures, not modules.** Namespace isolation means the
   consumer's Postgres and the library's Postgres are *different modules*; whether they land on **one
   container or two** is a fixture-scope/identity question, not a namespace one. The correct default
   is **isolate → possibly a duplicate container**, because accidental sharing is nondeterministic and
   this is a correctness-first system. Sharing one container is an **explicit opt-in optimization**
   (inject a provider, or a shared-scope fixture), never something that happens behind the author's
   back.

> **Isolate names, share resources — both deliberately.** Do not let "isolate the surface" get
> confused with "duplicate the world."

## The escape-hatch ordering

Three composition modes, in the order an author should reach for them:

1. **Bundled + isolated (default).** The library privately depends on its providers; the consumer sees
   nothing. Encapsulated and correct. Requires the per-plugin dep map above.
2. **Peer / explicit (fallback).** The consumer adds the provider to their *own* `[plugins]` and the
   library documents that it needs it. Correct, always valid, but leaky and verbose. This is close to
   today's behavior and needs no engine change — a legitimate way to ship topology-libraries *before*
   isolation lands. Because explicit-peer stays permanently valid (isolation adds a terser default, it
   does not invalidate explicit), shipping peer now is **not** a walk-back — just interim boilerplate.
3. **Injection (advanced, for deliberate sharing).** The library takes a provider *handle* the
   consumer supplies (`topology(ctx, { store = t:use(my_pg) })`), so library and consumer share one
   resource on purpose. This is the shared-singleton case, and the only one where the consumer *should*
   know about the dependency.

## Contract drift (the one residual hazard)

Two plugins both providing a `postgres` surface with *different* shapes. Handle it in two stages:

- **Now — loose / by convention.** The provider owns its surface; consumers use it as documented; a
  duplicate-provider collision is a clear load error.
- **As first-party names stabilize — kernel-blessed interfaces.** For the canonical few (`postgres`,
  `redis`, `kafka`), the kernel defines the *contract* the way it already defines what `docker` means
  and **refuses redefinition** (`is_builtin_capability`). Providers must satisfy the blessed shape.
  This is the principled endgame: **kernel-defined capability interfaces, plugin-supplied
  implementations** — dependency inversion with the kernel as the interface registry.

## Amendment to plugin-system.md

§ Resolution currently ends the "Intra-plugin `require`" note with: *"plugins vendor their helpers;
there is no dependency resolver."* Replace with: *"plugins vendor their **helpers** (intra-plugin
requires, by canonical name); **inter-plugin** dependencies are declared in `[dependencies]` and
resolved privately per plugin — see [plugin-composition.md](plugin-composition.md)."*

## Status

- **Model:** settled (this doc).
- **Ships without engine work:** mode 2 (peer/explicit) — unblocks `prova-mocks` topologies now.
- **Engine change (single owner):** per-plugin dependency map + plugin-scoped `require` binding +
  `requires` propagation, in `plugins.rs`. Pinned first by executable proofs (red), including a
  **negative** proof that a library's inner dependency is *not* reachable from the consumer's
  namespace — encapsulation is proven by inaccessibility, which is easy to forget to test.
- **Later:** kernel-blessed capability interfaces for the canonical providers.
