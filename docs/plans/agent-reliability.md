---
title: Agent reliability — falsifiers, claims, and what is still owed
type: plan
maturity: in-flight
tags: [plan, specs, falsify, claims, owed, agents]
---

# Agent reliability

> Prova is a TDD framework for agents. An agent can *say* it implemented a prose spec and not
> have; it can *say* a feature works when it doesn't. Every atom here replaces a sentence an agent
> could say with an artifact it must produce.

## The taxonomy this plan works from

Each row is a way to be wrong while sounding right. Three are closed, three are not.

| What an agent says | How it's false | Atom | State |
|---|---|---|---|
| "I implemented the spec" | never ran it | `spec` → `burndown` → graduation | ✅ shipped |
| "The tests pass" | they pass vacuously | `falsified_by` → `prova falsify` | ✅ this branch |
| "The doc says X, I built X" | implementation diverged | `covers` → `prova owed` | ✅ this branch |
| **"0 failed"** | **everything skipped** | **the run record** | ❌ not built |
| **"That failure is pre-existing"** | **it isn't** | **`prova --since <ref>`** | ❌ not built |
| "It works" | shown with a bespoke script, not the bar | — | ❌ no design |

## Landed on this branch (6 commits, unpushed)

- **Falsifiers** — `falsified_by = function(t) … end`, `prova falsify` applies the mutation and
  inverts the verdict. A body that survives is *vacuous*. A falsifier that raises fails as a
  **mutation** failure, so a broken falsifier can't masquerade as a body that correctly went red.
- **Claims + `prova owed`** — `<!-- claim: id -->` anchors, `covers = "path#id"`, one ledger over
  every origin (`UNBOUND` / `UNPROVEN` / `SPEC`; duplicate id is the only error).
- **Pinning** — `prova owed --pin` records the claim's normalized text; an edit reports `STALE`.
- **Readiness fix** — the port probe now requires the container listening **and** the host mapping
  accepting. Neither alone is a true signal: Docker Desktop's proxy accepts early, OrbStack's
  refuses late.

Each was authored spec-first and graduated in place. `learn` topics: `falsify`, `claims`.

## Do this first: rebase onto main

**This branch is based on `ci-win-proofs-gate`, not `main`**, and `main` has since taken
`feat(globals)!: canonical prova.* + declared [globals] inject` — a deliberate breaking change.
Expect two kinds of work:

1. **Lua**: the new proofs (`proofs/spec/engine/falsify_test.lua`, `claims_test.lua`) and the
   placement suite use bare `shell` / `fs` / `json` / `socket`. Under the injection model these
   want `prova.*` or a `[globals]` declaration.
2. **Rust**: `engine.rs`, `modules.rs` and `prova-cli/src/main.rs` are touched by both lines.

Rebase before building anything new on top, or the conflict surface only grows.

## Next, in priority order

### 1. The run record — what was NOT run

The highest-value remaining atom, because it closes the one lie that survives an *honest* agent:
"0 failed" is technically true when everything skipped. Real case from this work: 34 placement
specs read as `SKIP` on a machine with no broker.

Emit a record per run — binary hash, commit, selection, counts, and **prominently the skipped and
deselected**. `prova attest <address>` then fails when a claimed obligation didn't actually
execute in it.

Open decision: a file prova writes (greppable, diffable in a PR) or state in `.prova/var/` a verb
queries (cleaner, invisible to anything that isn't prova). **Do not sign it** — the threat model
is a careless agent, not a malicious one, and signing would be theater plus key management.

### 2. `prova --since <ref>` — introduced or pre-existing

On failure, re-run the failing selection against the merge-base build and label each one. Done by
hand twice during this work; both times it changed the report from a guess into a fact, and once
it stopped a wrong "that's pre-existing" claim from being made.

### 3. `prova owed` over MCP

`falsify` already rides the `run` selection. `owed` has no MCP surface, so an agent driving prova
over MCP cannot see what a package owes. The CLI path resolves the manifest differently from
`McpEnv::resolve_call`; wire it deliberately rather than bolting it on.

### 4. Suite preconditions (speculative)

Nothing detects an obligation a *suite* assumes but no doc states — the placement conformance
suite silently required the broker to offer a slot kind it never documented. Partial answer: let a
suite declare its preconditions so `owed` can list them. Build only after the anchor mechanism has
proven itself.

## Principles that constrain all of it

- **Opt-in, but rewarded.** `prova.test(name, fn)` must just work, forever. `[claims]` absent ⇒
  the subsystem is inert. A pin is per-*binding*, not a global switch. Machinery agents route
  around is worse than none.
- **Report, don't nag.** Prova reports; the *prompting* lives in `skill.md`, so it stays tunable
  per team without touching the binary.
- **Anchored claims only.** Inferring obligations from unmarked prose is how traceability becomes
  ritual and gets abandoned.
- **The honest ceiling** is "an agent cannot be wrong by *accident*", not "cannot be wrong". A
  determined agent can write a trivial falsifier. Stacking more atoms has diminishing returns.

## Gotchas worth not rediscovering

- `cargo xtask proofs`, never a bare `prova` — that's whatever was installed last.
- `learn` topics are hard-capped at **90 rendered lines** by a unit test. A new capability gets its
  own topic; it does not swell an existing one.
- Adding a capability means five places: engine, CLI verb, `learn` topic, `skill.md`, **MCP**.
  Miss the last and agents driving over MCP cannot reach it.
- `proofs/spec/tap/` needs a live redis container and is **not** `requires`-gated on docker, so it
  fails on any machine where that container can't serve. Worth gating.
