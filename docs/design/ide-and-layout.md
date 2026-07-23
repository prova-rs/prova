# Project Layout & IDE Integration

How a prova package is laid out on disk, and how editor completion + type-checking "just work" for
test authors and plugin authors alike — with zero manual wiring.

## The insight this is built on

Lua is a *dependency* of a prova package, rarely its primary language. A test author drops a few
`*_test.lua` files into an otherwise-Rust/Go/TypeScript service repo and wants their editor to
understand `prova.test`, `t:expect`, and `require("postgres")` — without learning LuaLS, editing a
`.luarc.json`, or checking out the prova source. So prova owns that setup, and scales how invasive it
is to how much the package has opted in.

Two facts drive the whole design:

1. **LuaLS binds to the workspace root you open**, and reads a `.luarc.json` there. prova writes that
   pointer into the **home directory** — which is the package root an editor attaches to.
2. **`---@meta <name>` makes `require("<name>")` resolve by module name** — decoupled from the file's
   path. That's what lets a plugin cached under a ref-hashed directory still be found as
   `require("postgres")`.

## The "prova home"

**Home = the project ROOT — the directory an editor opens and where `proofs/` live — and it is the
base for everything.** Every manifest-relative key (`proofs`, `config`, `plugin_root`) and every
generated artifact (`.luarc.json`, `running/`) resolves against it. A package picks one of four
manifest locations:

| Manifest location | Home (root) | Feel |
|---|---|---|
| `prova.toml` | the dir holding it | flat — zero nesting |
| `.prova.toml` | the dir holding it | flat, hidden — one manifest, out of sight |
| `prova/prova.toml` | the dir **above** `prova/` | visible nested — config tucked in `prova/` |
| `.prova/prova.toml` | the dir **above** `.prova/` | hidden nested — config tucked in `.prova/` |

The nested forms are how a package hides prova's own files — the manifest, `config.lua`, `plugins/` —
inside a `prova/` or `.prova/` **nook**, while the ROOT stays the parent: where `proofs/` live in the
open, and where an editor attaches. So `config = ".prova/config.lua"` and `proofs = ["proofs"]` in a
`.prova/prova.toml` resolve to `<root>/.prova/config.lua` and (via discovery) `<root>/proofs` — both
relative to the root, never to the manifest's own directory. One base, one mental model, a layout an
agent reads off a single file. A package stays a **relocatable unit**: home is the root whether the
manifest sits flat at the root or tucked into a nook — the only difference is that `config`/
`plugin_root` point *into* the nook for the nested form.

Discovery walks **up** from the current directory (like git finding `.git`), so `prova` runs from
anywhere inside a package — including from inside the nook itself. Two rules:

- **Exactly one of the four variants per directory.** Two in one directory is an ambiguous layout;
  prova refuses to guess. (Across *different* levels is fine — see nested packages below.)
- **The nearest manifest wins, and a deeper one is its own package.** A `prova.toml` further down the
  tree is an independent package, not a child of an ancestor's — running from inside it resolves it,
  not the ancestor, and its `proofs/` are never swept into the ancestor's discovery.

The signal for "nested" is the manifest **filename**, not the directory name alone: only a *bare*
`prova.toml` inside a directory named `prova`/`.prova` is nested (which reserves those two directory
names for that role, and makes discovery from inside the nook resolve the same root as discovery from
outside it). A hidden `.prova.toml` is a flat file whatever its directory is called, so it never
hoists — the escape hatch is exact: to root a *flat* package in a directory literally named
`prova`/`.prova`, use `.prova.toml`. The one on-disk ambiguity — a bare `prova.toml` in such a
directory — is decided by the reservation, deterministically, wherever you run prova from.

Because the root *is* the editor-attach point, `.luarc.json` needs no special-casing: it goes at home,
the same directory everything else resolves against. `prova ide setup` writes it at the discovered
home, so running it at the top wires the top and running it in a nested package wires that package. A
package with no setup of its own inherits the nearest ancestor's `.luarc.json` (LuaLS walks up);
running setup there drops a more-specific one that LuaLS then prefers — self-correcting exactly when
someone asks.

A typical hidden-flat layout (what prova itself uses):

```
myproject/
├── .prova.toml          ← the manifest — home IS the root
├── .luarc.json          ← the editor pointer, at the root LuaLS binds to
├── proofs/              ← visible tests (proofs = ["proofs"], the default)
└── .prova/              ← just a nook for config + plugins the manifest points into
    ├── config.lua       #   config = ".prova/config.lua"
    └── plugins/         #   plugin_root = ".prova/plugins"
```

The nested form (`.prova/prova.toml`) is the same picture with the manifest moved into the nook — the
root, and `proofs/` at it, are unchanged:

```
myproject/                ← still the home (root)
├── .luarc.json          ← still here, at the root
├── proofs/              ← still here, discovered from the root
└── .prova/
    ├── prova.toml       ← the manifest now lives in the nook
    ├── config.lua       #   config = ".prova/config.lua"
    └── plugins/         #   plugin_root = ".prova/plugins"
```

Nothing else is generated inside the package. `.luarc.json` names its annotation sources directly:

```json
"workspace.library": [
  "~/.local/share/prova/lua/annotations",                     // core stubs: ONE stable dir, .version-stamped
  "~/.cache/prova/plugins/…prova-postgres/tag-main/library"   // the checkout itself
]
```

```
~/.local/share/prova/
└── lua/annotations/                 ← core stubs: one STABLE dir shared by every project; a
    ├── prova.lua                      `.version` stamp refreshes contents on upgrade, so the
    ├── modules.lua                    `.luarc.json` entry is written once and never churns
    └── .version
~/.cache/prova/
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
package has opted into prova:

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

Renders a catalog archetype into the current directory, then wires IDE support as a finishing step.
The archetype (not a flag) owns the layout — where `prova.toml` lands, what the proof dir is named —
so `prova init <key>` scaffolds whatever that entry produces; `--no-ide` (alias `--no-luals`) skips
the wiring. The catalog is prova's built-ins `project` and `plugin` plus `[init.*]` from `~/.config/prova/config.toml`;
`prova init` with no key picks from it interactively, `prova init --list` prints it. It refuses to run
if any manifest location already exists — it never clobbers an existing layout. IDE wiring itself is
also available on its own as `prova ide setup` (see above).

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

Open the package in any LuaLS-backed editor, and `require("postgres")` completes — `pg.client:
query_value(...)` is typed, wrong argument counts are flagged. The test author did nothing but
declare the plugin.
