# init — scaffold a package from the archetype catalog

`prova init [<key>]` renders a catalog archetype into the current directory, then wires LuaLS
IDE support. It NEVER overwrites: any existing manifest aborts the render, unless the entry
declares `in_package = "allow"` (it augments a package rather than creating one). No key + a
terminal → interactive select; no key + no terminal → error (never a hang), so in automation
always name the key.

## The catalog, on this machine

{{init_catalog}}

## Which key, when

| You are... | Key |
|---|---|
| Adding proofs to an app/repo (the common case) | `project` |
| Authoring a reusable package — a namespace others `require()` | `package` |
| Adding a local package to THIS one (run it inside the package) | `package` — it lands in the `packages` dir |
| In an org with its own entries (see below) | the org's key |

## Flags that matter in automation

```
prova init project --headless                # the project scaffold is promptless — this just works
prova init package --headless -a name=redis   # package: `name` has no default, so answer it
# the flags, separately:
#   --headless        never prompt; an unanswerable prompt is an ERROR
#   -a k=v            --answer, repeatable; beats baked answers
#   --defaults        take the archetype's default for every remaining prompt
#   -s ci             --switch, repeatable (e.g. `-s standalone` forces the standalone repo shape)
```

Answer precedence: CLI `--answer` > the entry's baked answers > injected package state > prompt
(or archetype default with `--defaults`).

## Package-state injection

Inside an existing package, every render also receives the `prova:in-package` switch plus
`prova_package_root` / `prova_packages_dir` answers — generic facts ANY archetype can read (the
`package` entry uses them to scaffold a local package into the `packages` dir instead of a standalone
repo). Outside a package none are supplied.

## Extending the catalog

`~/.config/prova/config.toml` layers `[init.*]` entries over the built-ins — a matching key
REPLACES the built-in outright; a new key adds. A `source` is anything archetect resolves: a git
URL (optionally `#ref`) or a local path.

```toml
[init.service]
description = "A service package pre-wired for postgres + http"
source      = "https://github.com/acme/prova-service-archetype.git#v1"
defaults    = true
in_package  = "allow"        # may render inside an initialized package (default: deny)
[init.service.answers]
proof_dir = "proofs"
```

This file is also where a team bakes ITS preferences into `project` — when a human says "use my
init defaults", this is where those live.

See also:
- `prova learn project` (what the scaffold writes, and where)
- `prova learn packages` (adding a capability after scaffolding)
- `prova learn package-authoring` (scaffolding a package for others instead)
