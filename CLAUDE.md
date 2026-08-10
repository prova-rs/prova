# CLAUDE.md

Guidance for agents and developers working in this repository.

## Project Overview

**Prova** is a programmable, language-agnostic **black-box acceptance-test runner** — a real
scripting language (Lua) plus a real fixture model, shipped as a single static binary. It brings a
system into existence (render it, build it, boot it), then probes it with shell + HTTP + filesystem
assertions, with fixtures holding setup and teardown together. Declaration files are
`*.prova.lua` (`*_test.lua` / `*.test.lua` accepted quietly — this repo's own suite still uses
them); a
`prova.toml` manifest declares what to run and how.

See `README.md` for the pitch and `docs/design/` for the durable design docs (foundations,
architecture, package system, topologies, mocks/proxies/drivers, agent ergonomics).

## Workspace Structure

```
crates/prova-core       # the engine: Lua DSL, fixtures/scopes, runner, reporters, package system
crates/prova-cli        # the `prova` binary — CLI, `prova init`/`ide setup`, and MCP mode
crates/prova-archetect  # the `archetect` render package (archetect-core embedded in-process)
xtask                   # build/install automation (this is the `cargo xtask` front door)
proofs/                 # prova's own black-box proofs — prova, proven by prova (dogfooding)
docs/design/            # durable design docs        docs/plans/  # in-flight plans
```

## Build & Quality

**Prova is this repo's quality interface, and any `prova` is the right one to type.** The
manifest's `[runner]` trampolines every invocation through this tree's freshly-built binary
(docs/design/manifest.md#manifest-declared-runner), so freshness and identity are mechanism —
the old "never prove through an installed prova" rule is retired. Ask the tool, not this file:
`prova learn project` (the card), `prova run --list` (the profiles), `prova switches` (the
opt-in classes).

```bash
prova                    # the black-box suite        prova --last-failed   # the inner loop
prova run ut             # unit tests, deputed via nextest into the account
prova run quality        # clippy -D warnings, unwrap/expect ratchet, file sizes
prova run coverage       # line coverage, ratcheted against the committed baseline
prova run all            # the pre-push sweep: proofs + ut + quality

# Cold start (no prova installed yet) — cargo is the artifact tool, once:
cargo run -p prova-cli --           # then `cargo xtask install` puts prova on PATH

# Artifacts (xtask's whole job)
cargo xtask install                 # install to ~/.cargo/bin; refreshes the user-scoped prova MCP
                                    # build — restart Claude Code afterward to load it
cargo xtask check / build / sweep   # cargo check · release build · drop stale target/ artifacts
```

Inside a proof, drive prova recursively through `prova.bin` (the runtime injects its own executable),
never a bare `prova`. `proofs/hermeticity/binary_identity_test.lua` fails the suite if one reappears.
Consumer repos are the opposite and correctly so: an archetype or package proves a *released* prova via
`prova-rs/run-action` at a pinned version, because what they must test is what users get.

**Formatting:** this tree is **not** blanket-`rustfmt`-clean — a repo-wide `cargo fmt` churns
unrelated files, so there is deliberately no `xtask fmt`. Match the surrounding style by hand; format
only the specific files you touched, if at all.

## Version Control

This repository uses **Jujutsu (jj)**, not git — never run `git` commands here.

It is **one jj repo with multiple workspaces** (`jj workspace list`), all sharing the one store in
`prova/.jj`. Commits are shared storewide; only each workspace's working-copy `@` differs.

**Workspace rule: one workspace per agent, never shared.** The interactive session works in the
`default` workspace (`prova/`). The other workspaces (`prova-agents/`, `prova-mocks/`, …) exist so
*concurrent* agents never fight over one working copy: a spawned/background agent claims its own
(`jj workspace add ../prova-<agent>`), works there, and its commits are visible storewide the
moment they're made. Do not treat any single side workspace as the shared place "where feature work
goes." Editing files by absolute path lands them in whichever workspace the path points at — stay
inside your own.

```bash
jj status        # working-copy changes        jj log            # history
jj commit -m ""  # seal @ and start a fresh empty @ on top
```

Do not push, move bookmarks, or squash without an explicit ask. (Signing is off here — this repo is
not under ~/work/.)

## Project Documentation

- `docs/design/` — durable design docs (the north star, architecture, ecosystem).
- `docs/plans/` — in-flight implementation plans; fold outcomes back into `docs/design/` when they land.
- `proofs/` — prova's own acceptance proofs; extend these when changing runtime behavior.
