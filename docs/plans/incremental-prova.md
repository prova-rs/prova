# Incremental prova — conduct identity, and not paying twice

Status: **drafted 2026-08-13, decisions open.** A joint design for three shelf items that are one
mechanism seen from three sides:

- `lifecycle.md#resumable-runs-incremental-verdicts` — a failed sweep re-pays the whole world;
- `manifest.md#subject-provisions-at-first-read` — a run pays for a subject no selected proof reads;
- `agent-ergonomics.md#dedupe-identical-deputy-conducts` — two deputies conduct the same cargo twice.

Companion to [shared-deputies.md](shared-deputies.md), whose `Scope.Run` store is the run-scoped
half of what this generalizes, and to [verifiers.md](../design/verifiers.md), whose conducts are
the expensive things being counted.

## The problem, measured

One `prova run all` on this tree, 2026-08-13 (829.3s wall, 636 proofs):

| Conduct | Wall |
|---|---|
| `cargo nextest run --workspace` (the ut deputy) | 319.6s |
| `cargo clippy --workspace --all-targets -- -D warnings` | 263.1s |
| `cargo clippy --workspace --lib --bins` (restriction lints) | 77.9s |
| 3 × `cargo build` (sandbox subjects) | ~54s |
| **everything else — all 636 proofs** | **~115s** |

**93% of a sweep is four cargo conducts, and the proofs themselves are two minutes.** Within one
run that is already as good as it gets: `Scope.Run` conducts each of those once. The waste is
*across* runs — a proof-only edit, a doc-only edit, a re-run after one ratchet regression re-pays
all 660s to re-ask questions whose inputs did not change. Six sweeps landed this session's work;
five of them re-earned identical answers for the ut lane.

The three shelf items are three faces of one missing fact: **prova does not know what a piece of
work depends on**, so it cannot tell "unchanged" from "unknown", and must re-do everything.

## The substrate: conduct identity

Give every expensive thing an **identity** — a content address over what determines its result:

```
identity(conduct) = digest(argv, cwd-relative, env allowlist, tool config, INPUTS)
```

Everything else falls out of that one fact:

- **Dedupe within a run** is identity-keyed slots instead of name-keyed ones. `Scope.Run` today
  keys the store by fixture *name*, which is why two differently-named deputies with identical
  conducts run twice. Key by identity and the second adopts the first's value — no new concept,
  one changed map key.
- **Resume / verdict replay across runs** is the same identity, persisted in `var/` with the
  verdict it earned. An unchanged identity replays; anything else re-earns.
- **Lazy provisioning** is a conduct whose identity is (build command, `sources` digest) and whose
  execution is deferred to first read. The `[runner]` provision stamp is already a crude,
  hand-rolled version of exactly this.

## The decision that governs everything: what counts as INPUTS

This is where the design can be wrong in a way that green-washes, so it is decision one.

- **(a) Author-declared** — `inputs = { "crates/**", "Cargo.lock" }` on the fixture/proof. Honest,
  auditable, and wrong only if the author lies. Burdens every author, and an omission is silent.
- **(b) Auto-derived, conservative** — the package's tracked-tree digest plus the binary
  fingerprint. Any change to any tracked file invalidates every verdict. Nearly useless for a
  monorepo edit… except that it exactly serves the case that hurts: **re-running after a failed
  sweep with no edits at all**, and re-running after a *doc-only* edit.
- **(c) Observed** — record what the conduct actually read (fs/proc tracing). Truest, most
  portable-hostile, and a research project.

**Recommendation: (b) first, (a) as an opt-in narrowing.** (b) is sound by construction — it can
only be over-conservative — and it captures the measured waste (a re-run after a red sweep). (a)
then lets the ut deputy declare `inputs = { "crates/**", "Cargo.*" }` so a proofs-only edit stops
re-paying 319.6s. (c) stays out of scope.

## The honesty rule: a replay is not a pass

A cached green that reads like a fresh green is a lie the tally tells. Non-negotiable:

- a replayed verdict reports as **REPLAYED**, counted separately in the tally line;
- `attest` treats a replay as evidence **only** when the identity includes the subject's binary
  fingerprint — the record already stores one;
- `--no-replay` (and a red run) re-earns everything;
- the identity's inputs are printed on demand, so "why did this replay?" is answerable.

## Increments

1. **Conduct identity + in-run dedupe.** Key the `Scope.Run` store by identity as well as name.
   Closes `dedupe-identical-deputy-conducts` with no persistence and no new failure modes.
   Cost: small. Value here: zero (one deputy); value in the fleet: a conduct per sweep.
2. **Run journaling + `--resume`.** Persist per-node outcomes with the run's identity; `--resume`
   re-executes only what was red, unattested, or absent, in the same tree state. No cross-run
   caching, therefore no soundness question — this is "finish the sweep you started".
   Cost: medium. Value: the 40-minutes-in failure stops costing 45 more.
3. **Verdict replay (b).** Persist verdicts against the conservative identity; replay on an
   unchanged tree. Cost: medium. Value: the no-edit re-run drops from 829s to ~120s.
4. **Declared inputs (a).** Opt-in narrowing, starting with the ut deputy. Cost: small once 3
   exists. Value: proofs-only and docs-only edits stop paying for cargo.
5. **Lazy provisioning.** The subject provisions at first `prova.bin` read: an `__index` seam
   feeding a CLI-provided provisioner, single-flighted under `-j`, taking the package's `cargo`
   lock (a lazy build racing a proof that holds `writes("cargo")` would violate the house rule the
   locks exist to enforce). Cost: medium-high — it is the one with a concurrency design in it.
   Value: `prova -k one_pure_proof` after an engine edit stops compiling the engine.

Increments 1–2 carry no soundness risk and can land without ratifying the INPUTS decision. 3–4 need
it. 5 is independent of 1–4 and can be scheduled on its own merits.

## Open questions for the principal

1. **INPUTS**: ratify (b)-then-(a), or start with (a) alone (more author burden, sharper results)?
2. **Ordering**: 2 → 3 → 4 (resume first, the safe win), or 1 first because it is cheapest?
3. **Scope of replay**: proofs only, or conducts too? A replayed *conduct* (skipping cargo while
   replaying its adopted junit) is where the 660s actually lives — and where a stale artifact would
   do the most damage.
4. **Does 5 belong here at all**, or as its own slice? It shares the identity substrate but nothing
   else, and its risk profile is concurrency, not staleness.
