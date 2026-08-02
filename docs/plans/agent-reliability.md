---
title: Agent reliability — falsifiers, claims, and what is still owed
type: plan
maturity: in-flight
tags: [plan, specs, falsify, claims, owed, record, attest, agents]
---

# Agent reliability

> Prova is a TDD framework for agents. An agent can *say* it implemented a prose spec and not
> have; it can *say* a feature works when it doesn't. Every atom here replaces a sentence an agent
> could say with an artifact it must produce.

## The taxonomy this plan works from

Each row is a way to be wrong while sounding right. Four are closed, two are not.

| What an agent says | How it's false | Atom | State |
|---|---|---|---|
| "I implemented the spec" | never ran it | `spec` → `burndown` → graduation | ✅ shipped |
| "The tests pass" | they pass vacuously | `falsified_by` → `prova falsify` | ✅ this branch |
| "The doc says X, I built X" | implementation diverged | `covers` → `prova owed` | ✅ this branch |
| "0 failed" | everything skipped | the run record → `prova attest` | ✅ this branch |
| **"That failure is pre-existing"** | **it isn't** | **`prova --since <ref>`** | ❌ not built |
| "It works" | shown with a bespoke script, not the bar | — | ❌ no design |

## Landed on this branch (unpushed)

- **Falsifiers** — `falsified_by = function(t) … end`, `prova falsify` applies the mutation and
  inverts the verdict. A body that survives is *vacuous*. A falsifier that raises fails as a
  **mutation** failure, so a broken falsifier can't masquerade as a body that correctly went red.
- **Claims + `prova owed`** — `<!-- claim: id -->` anchors, `covers = "path#id"`, one ledger over
  every origin (`UNBOUND` / `UNPROVEN` / `SPEC`; duplicate id is the only error).
- **Pinning** — `prova owed --pin` records the claim's normalized text; an edit reports `STALE`.
- **Readiness fix** — the port probe now requires the container listening **and** the host mapping
  accepting. Neither alone is a true signal: Docker Desktop's proxy accepts early, OrbStack's
  refuses late.
- **The run record + `prova attest`** — see below.

Each was authored spec-first and graduated in place. `learn` topics: `falsify`, `claims`, `record`.

Worth recording about the method: the record suite was written as twelve `spec`-flagged proofs
before any implementation, and one of them — that recording stays inert to the ordinary path —
came back a **failure** rather than an open spec, because it asserted something already true.
Prova refused to let a true statement sit behind a spec flag and demanded graduation. That is the
`spec` atom catching a misuse of itself, in the middle of building the next atom.

## Rebase onto main — DONE

The branch was based on `ci-win-proofs-gate`; it now sits on `main`. Three files conflicted, all
for one reason: **both lines independently split `falsify` into its own `learn` topic**. Resolved
as the union of the two alias sets, main's pointer form in `specs.md`, and the branch's richer
three-step discovery test.

The anticipated Lua work did not exist — the injection model added `prova.*` as canonical
*alongside* the bare built-ins (`modules.rs` still installs `shell`, `fs`, `json`, `socket` as
globals), so no proof needed rewriting. What did need fixing was portability: the claims suite was
written before main's portable-primitives migration and still used `2>&1` and a shelled-out
`mkdir -p`, neither of which survives the Windows proof gate.

## The run record — DONE (this branch)

Every run writes `.prova/var/last-run.json`: counts, provenance, the selection, and — named one by
one rather than summed — the skipped (with the gate's own reason) and the deselected.
`--record <path>` also emits it where CI can keep it. `prova attest <doc.md#id>` reconciles one
obligation against it and exits non-zero unless a covering proof actually executed and passed.

Two engine changes were needed. Deselected leaves emit no event, so `narrow_plan` returns their
paths alongside the count — **the count stays leaf-based** (a flow is one scheduling unit however
many steps it has; changing that broke five selection tests and was the one real regression in this
work). And leaf addresses are qualified by their file's stem through one shared
`prova_core::qualify_leaf_path`, because the record is read outside the run that wrote it and two
files' same-named tests must not have one's pass vouch for the other's skip.

Not signed, per the standing decision. Not a gate: recording never changes a verdict, output or
exit code, and a proof holds that.

## Next, in priority order

### 1. `prova --since <ref>` — introduced or pre-existing

On failure, re-run the failing selection against the merge-base build and label each one. Done by
hand twice during this work; both times it changed the report from a guess into a fact, and once
it stopped a wrong "that's pre-existing" claim from being made.

### 2. `prova owed` over MCP

`falsify` already rides the `run` selection, and `attest` now has its own MCP tool — so the
manifest-resolution seam it needed (`McpEnv::resolve_call` → `collect_obligations`) is built and
proven. `owed` is the same shape and should follow it rather than inventing a second path.

### 3. Suite preconditions (speculative)

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
- `cargo check` does not compile `#[cfg(test)]` bodies. Adding a field to a public struct passed
  both `check` and the whole proof suite, and still failed `cargo test` on one test-only
  initializer. Run `cargo test`, not `check`, before believing a core type change is clean.
- Piping a build into `tail` hands you **tail's** exit code, not cargo's, and truncates away the
  crate whose tests you actually changed. Redirect to a file and grep it.
