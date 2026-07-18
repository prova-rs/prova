# Project Layout & IDE Integration

How a prova project is laid out on disk, and how editor completion + type-checking "just work" for
test authors and plugin authors alike — with zero manual wiring.

## The insight this is built on

Lua is a *dependency* of a prova project, rarely its primary language. A test author drops a few
`*_test.lua` files into an otherwise-Rust/Go/TypeScript service repo and wants their editor to
understand `prova.test`, `t:expect`, and `require("postgres")` — without learning LuaLS, editing a
`.luarc.json`, or checking out the prova source. So prova owns that setup, and scales how invasive it
is to how much the project has opted in.

Two facts drive the whole design:

1. **LuaLS binds to the workspace root you open**, and reads a `.luarc.json` there. So the *pointer*
   must live at the project root, even if everything else prova owns is tucked into a subdirectory.
2. **`---@meta <name>` makes `require("<name>")` resolve by module name** — decoupled from the file's
   path. That's what lets a plugin cached under a ref-hashed directory still be found as
   `require("postgres")`.

## The "prova home"

**Home = the directory that contains `prova.toml`.** Every manifest-relative path (`[run] paths`)
resolves against it. A project picks one of three locations, trading root clutter for tidiness:

| Manifest location | Home dir | Feel |
|---|---|---|
| `prova.toml` | project root | flat — zero nesting |
| `prova/prova.toml` | `prova/` | visible — tests + config in one navigable dir |
| `.prova/prova.toml` | `.prova/` | hidden — config + generated files out of sight |

Discovery walks **up** from the current directory (like git finding `.git`), so `prova` runs from
anywhere inside a project. Two rules keep it unsurprising:

- **More than one location present → hard error.** The layout is ambiguous; prova refuses to guess.
- **Name-based root derivation.** A `prova.toml` in a directory literally named `prova`/`.prova` is
  treated as a nested home whose real root is the *parent* — so `cd prova && prova` resolves the same
  `(root, home)` as running from the project root. (Without this, discovery from inside the home dir
  would mistake it for a flat project and drop a second `.luarc.json` in the wrong place.)

A typical hidden layout:

```
myproject/
├── .luarc.json          ← the only generated file in the tree (LuaLS binds at the ROOT)
└── .prova/              ← home
    ├── prova.toml       # committed
    └── suites/          # committed tests (paths = ["suites"])
```

Nothing else is generated inside the project. `.luarc.json` names its annotation sources directly:

```json
"workspace.library": [
  "~/.cache/prova/annotations/0.2.10",                        // core stubs: one per VERSION
  "~/.cache/prova/plugins/…prova-postgres/tag-main/library"   // the checkout itself
]
```

```
~/.cache/prova/
├── annotations/<version>/           ← core stubs, shared by EVERY project on the machine
│   ├── prova.lua
│   └── modules.lua
└── plugins/<url>/<ref>/library/     ← the plugin checkout, fetched once, shared by all projects
```

## No per-project state outside the project

This is the load-bearing property. Look at what a project's annotations consist of: core stubs that
are byte-identical for a given prova version, and plugin stubs that live in checkouts shared by every
project using them. **The only project-specific fact is *which* plugins are used — and `prova.toml`
already records that, inside the repo.**

So prova stores nothing per-project outside the project. Both cache directories are keyed by things
that aren't projects (a version, a plugin ref), and both are bounded accordingly: by how many prova
versions you've installed, and by how many distinct plugin refs you've fetched — never by how many
projects you have.

That kills an entire class of problem rather than managing it. An earlier iteration bundled each
project's selection into a per-project "view" directory in the cache. It bought nothing — every
element in it was already shared — but the moment a cache directory is keyed by a project, the cache
knows something the project doesn't, and you need a back-pointer plus a garbage collector to notice
when that project disappears. Referencing shared paths directly means **nothing can be orphaned, so
there is nothing to collect**: no marker file, no GC pass, no slug, no eviction policy.

Two consequences worth stating:

- **A repo carries no generated annotation files at all.** The only project-local artifact is
  `.luarc.json`. It holds machine-local absolute paths, so it isn't shareable and shouldn't be
  committed — `prova init` says so once and leaves the `.gitignore` decision to the user. prova never
  edits a user's `.gitignore`.
- **Plugin stubs are referenced, not copied.** Editing a plugin's `library/` is visible to the editor
  immediately, with no re-sync step.

### The cost: the entry list tracks the plugin set

The direct list isn't free. Because the entries *are* the plugin set, adding or removing a plugin
changes the list — so `.luarc.json` has to be rewritten, where the old design could leave a single
stable pointer alone forever. prova handles that per ownership:

| Situation | Behavior |
|---|---|
| prova wrote the file (it carries exactly prova's three keys) | Rewritten each run — always current |
| `manage = "always"` | Merged each run: stale prova-managed entries swept, current ones added |
| `manage = "auto"` + a user-owned file | Left alone; prints the `prova init` hint when entries are missing |

Ownership is decided by exact key-set match, which errs toward "not ours": add a single setting of
your own and prova treats the file as yours from then on. The sweep only reclaims entries under
prova's own cache roots — a plugin resolved from a **local path** is indistinguishable from a
hand-added entry, so dropping one leaves its (still valid) path in the list for the user to remove.
That asymmetry is deliberate: leaving a stale entry is a much cheaper mistake than deleting one the
user added.

### Why absolute paths, not `${env:HOME}`

LuaLS does expand a few placeholders server-side (`${env:NAME}`, `${workspaceFolder}`, `${3rd}`,
`${addons}`), so a relocatable pointer is technically possible. prova emits absolute paths anyway:
`.luarc.json` is generated per-machine and never committed, so portability buys nothing, while an
unset variable or an unrecognized placeholder expands to empty **silently** — a library path that
doesn't exist and completions that quietly stop working. A path computed from the resolved layout is
correct by construction, including under `XDG_CACHE_HOME`. Drift is handled by regeneration: every
manifest-backed run refreshes the stubs and re-checks the pointer.

## The `.luarc.json` pointer — a graceful-degradation ladder

Only the pointer is gated (the core stubs are always installed). The gate keys on how much the
project has opted into prova:

| Situation | Behavior |
|---|---|
| **No manifest** (ad-hoc `prova foo_test.lua`) | Polite: no `.luarc.json`, nothing dropped in cwd. |
| **Manifest, no `[luals]`** | `auto` (below). |
| **`[luals] manage = "…"`** | Obey exactly. |

`[luals] manage` values:

- **`auto`** (default) — create `.luarc.json` when absent; refresh it when prova wrote it; when the
  *user* owns one, leave it and print a one-line hint to run `prova init`. This auto-detects project
  type: a non-Lua project (Lua present only for prova) has no `.luarc.json`, so prova sets it up and
  keeps it current; a Lua-native project already owns one, so prova stays a polite guest.
- **`always`** — create, or non-destructively reconcile our entries into an existing file: stale
  prova-managed entries swept, current ones added, `runtime.version` set only if unset. Never touches
  the user's other LuaLS settings or their own library entries; a non-JSON (commented) existing file
  is an error with a hint, never a corruption.
- **`never`** — never touch `.luarc.json`. The core stubs are still installed; the user wires the
  pointer themselves.

## `prova init`

Scaffolds `prova.toml` + the home dir + a root `.luarc.json`. `--hidden`
uses `.prova/`, `--flat` puts the manifest at the root, `--no-luals` skips IDE wiring (and sets
`[luals] manage = "never"`). It refuses to run if any manifest location already exists — it never
clobbers an existing layout.

## The plugin side

A plugin ships a **`library/<name>.lua`** file — a `---@meta <name>` stub declaring
`<name>.container(ctx, opts)`, the resource shape, and the client's methods. It's the consumer-facing
contract, separate from the implementation (mirroring how prova's own core stubs are separate from
the Rust engine). The plugin archetype generates it from the same prompts that generate the client,
so every plugin is IDE-ready by construction; `prova plugin lint` warns (non-fatally) when a plugin
ships without one.

A plugin author gets IDE support for their *own* source the same way a consumer does: running the
plugin's self-test (`prova` against its `prova.toml`) writes a `.luarc.json` listing the core stubs
plus the plugin's own `library/`. Only `.luarc.json` lands in the repo (and should be gitignored); the
shipped `library/<name>.lua` is committed.

## The end-to-end "just works" flow

```
# In any project:
prova init                                   # (or just run prova with a hand-written prova.toml)

# prova.toml:
[plugins]
postgres = "prova-rs/prova-postgres@v0.2.0"

prova                                        # fetches the plugin, syncs its stub, runs tests
```

Open the project in any LuaLS-backed editor, and `require("postgres")` completes — `pg.client:
query_value(...)` is typed, wrong argument counts are flagged. The test author did nothing but
declare the plugin.
