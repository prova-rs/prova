# CLAUDE.md

Guidance for agents and developers working in this repository.

## Project Overview

**Prova** is a programmable, language-agnostic **black-box acceptance-test runner** — a real
scripting language (Lua) plus a real fixture model, shipped as a single static binary. It brings a
system into existence (render it, build it, boot it), then probes it with shell + HTTP + filesystem
assertions, with fixtures holding setup and teardown together. Tests are `*_test.lua` files; a
`prova.toml` manifest declares what to run and how.

See `README.md` for the pitch and `docs/design/` for the durable design docs (foundations,
architecture, plugin system, topologies, mocks/proxies/drivers, agent ergonomics).

## Workspace Structure

```
crates/prova-core       # the engine: Lua DSL, fixtures/scopes, runner, reporters, plugin system
crates/prova-cli        # the `prova` binary — CLI, `prova init`/`ide setup`, and MCP mode
crates/prova-archetect  # the `archetect` render plugin (archetect-core embedded in-process)
xtask                   # build/install automation (this is the `cargo xtask` front door)
proofs/                 # prova's own black-box proofs — prova, proven by prova (dogfooding)
docs/design/            # durable design docs        docs/plans/  # in-flight plans
```

## Build & Development Commands

Prova uses the same `cargo xtask` automation as archetect. Prefer it over raw cargo.

```bash
# Install the `prova` binary to ~/.cargo/bin (this is the canonical install path).
# Also how you refresh the user-scoped prova MCP build — restart Claude Code afterward to load it.
cargo xtask install                 # add --static-openssl=false to skip static OpenSSL

# Run prova without installing
cargo xtask run -- init --list      # == cargo run -p prova-cli -- init --list

# Tests
cargo xtask test                    # whole workspace (some integration tests need a Docker daemon)
cargo xtask test-crate prova-core   # a single crate

# Check / lint / build / GC
cargo xtask check                   # cargo check --workspace --all-targets
cargo xtask clippy                  # clippy with -D warnings
cargo xtask build                   # release build
cargo xtask sweep                   # drop stale target/ artifacts (auto-installs cargo-sweep)
```

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
