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
| Authoring a reusable plugin — a namespace others `require()` | `plugin` |
| Adding a plugin to THIS package (run it inside the package) | `plugin` — it lands in `plugin_root` |
| In an org with its own entries (see below) | the org's key |

## Flags that matter in automation

```
prova init project --headless                # the project scaffold is promptless — this just works
prova init plugin --headless -a name=redis   # plugin: `name` has no default, so answer it
# the flags, separately:
#   --headless        never prompt; an unanswerable prompt is an ERROR
#   -a k=v            --answer, repeatable; beats baked answers
#   --defaults        take the archetype's default for every remaining prompt
#   -s ci             --switch, repeatable (e.g. `-s standalone` forces the plugin repo shape)
```

Answer precedence: CLI `--answer` > the entry's baked answers > injected package state > prompt
(or archetype default with `--defaults`).

## Package-state injection

Inside an existing package, every render also receives the `prova:in-package` switch plus
`prova_package_root` / `prova_plugin_root` answers — generic facts ANY archetype can read (the
`plugin` entry uses them to scaffold a local plugin into `plugin_root` instead of a standalone
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

Go deeper: `prova learn project` (what the scaffold gave you) · `prova learn pdd` (what to do next).
