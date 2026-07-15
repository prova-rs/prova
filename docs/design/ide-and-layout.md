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

**Home = the directory that contains `prova.toml`.** Every manifest-relative path (`[run] paths`, the
generated `annotations/`) resolves against it. A project picks one of three locations, trading root
clutter for tidiness:

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
├── .luarc.json          ← the only thing at the project ROOT (LuaLS binds here)
└── .prova/              ← home
    ├── prova.toml       # committed
    ├── annotations/     # GENERATED, gitignored: core + plugin ---@meta stubs
    │   ├── prova.lua
    │   ├── modules.lua
    │   └── plugins/postgres.lua
    └── suites/          # committed tests (paths = ["suites"])
```

## Annotation sync

On every run that has a manifest (not read-only `--list`, not an ad-hoc explicit-path invocation),
prova refreshes `<home>/annotations/`:

- the **core stubs** (`prova.lua`, `modules.lua`), embedded in the binary, and
- each **resolved plugin's** `library/*.lua`, copied into `annotations/plugins/`.

The folder is recreated clean each run, so a plugin dropped from `prova.toml` loses its stub too. It
is prova-owned and self-ignored via an `annotations/.gitignore` of `*` — prova never edits any of the
user's own `.gitignore` files. Because the folder is stable and refreshed automatically, adding a
plugin makes its completions appear on the next run with **no `.luarc.json` change** — the pointer
never moves, only the contents.

## The `.luarc.json` pointer — a graceful-degradation ladder

Only the pointer is gated (the annotation folder always syncs). The gate keys on how much the project
has opted into prova:

| Situation | Behavior |
|---|---|
| **No manifest** (ad-hoc `prova foo_test.lua`) | Polite: no `.luarc.json`, nothing dropped in cwd. |
| **Manifest, no `[luals]`** | `auto` (below). |
| **`[luals] manage = "…"`** | Obey exactly. |

`[luals] manage` values:

- **`auto`** (default) — create `.luarc.json` when absent; when one already exists, leave it and
  print a one-line hint to run `prova init`. This auto-detects project type: a non-Lua project (Lua
  present only for prova) has no `.luarc.json`, so prova sets it up; a Lua-native project already owns
  one, so prova stays a polite guest.
- **`always`** — create, or non-destructively merge our two keys (`workspace.library += <home>/
  annotations`, `runtime.version = "Lua 5.4"`) into an existing file. Never touches the user's other
  LuaLS settings; a non-JSON (commented) existing file is an error with a hint, never a corruption.
- **`never`** — never touch `.luarc.json`. The annotation folder still syncs; the user wires the
  pointer themselves.

## `prova init`

Scaffolds `prova.toml` + the home dir + `annotations/` + a root `.luarc.json` in one step. `--hidden`
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
plugin's self-test (`prova` against its `prova.toml`) syncs the core stubs plus the plugin's own stub
into `annotations/` and writes a `.luarc.json`. Both the generated `annotations/` and `.luarc.json`
are gitignored; the shipped `library/<name>.lua` is committed.

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
