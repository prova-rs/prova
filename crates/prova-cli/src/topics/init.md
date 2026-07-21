# init — scaffold a package from the archetype catalog

`prova init [<key>]` renders a catalog archetype into the current directory, then wires LuaLS
IDE support. It NEVER overwrites: any existing manifest aborts the render. No key + a terminal →
interactive select; no key + no terminal → error (never a hang), so in automation always name
the key.

## The catalog, on this machine

{{init_catalog}}

## Which key, when

| You are... | Key |
|---|---|
| Adding proofs to an app/repo (the common case) | `default` |
| Authoring a reusable plugin — a namespace others `require()` | `plugin` |
| In an org with its own entries (see below) | the org's key |

## Flags that matter in automation

```
prova init default --headless                 # never prompt; unanswerable prompt = error
prova init default -a project_name=orders     # --answer k=v, repeatable; beats baked answers
prova init default --defaults                 # take the archetype's default for the rest
prova init default -s ci                      # --switch, repeatable
```

Answer precedence: CLI `--answer` > the entry's baked answers > prompt (or archetype default
with `--defaults`).

## Extending the catalog

`~/.config/prova/config.toml` layers `[init.*]` entries over the built-ins — a matching key
REPLACES the built-in outright; a new key adds. A `source` is anything archetect resolves: a git
URL (optionally `#ref`) or a local path.

```toml
[init.service]
description = "A service package pre-wired for postgres + http"
source      = "https://github.com/acme/prova-service-archetype.git#v1"
defaults    = true
[init.service.answers]
proof_dir = "proofs"
```

This file is also where a team bakes ITS preferences into `default` — when a human says "use my
init defaults", this is where those live.

Go deeper: `prova learn project` (what the scaffold gave you) · `prova learn pdd` (what to do next).
