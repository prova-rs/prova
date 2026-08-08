# The obligation lifecycle — the levels of Prova, and the vocabulary for them

> Companion to [`proof-driven-development.md`](proof-driven-development.md). That doc is the
> thesis: done is not a claim, it's a proof that runs. This doc is the *operational* shape of
> that thesis — where an obligation comes from, how far it has travelled, what you write and
> what you run at each level, and what the whole account should be called.

## One line

**The doc *claims* it; a proof *promises* it; the implementation *proves* it; the run *attests*
it.** Every capability Prova has added answers one question about one obligation — has this
travelled far enough to be believed? — and the whole journey is four verbs in one grammar. The
atoms were built one at a time; this doc is the map they turned out to form.

## Where an obligation comes from

Three origins, and the distinction is not decoration — it decides who can retire it.

| Origin | Written as | Retired by |
|---|---|---|
| **Implicit** — someone wrote a test | `prova.test(name, fn)` | deleting the test |
| **Deferred** — scoped, not yet built | `promises = "<reason>"` | keeping the promise, then graduating the flag |
| **External** — stated in prose or a ticket | `<!-- claim: id -->` + `covers` | a proof that discharges it |

An external obligation is the only one that can outlive everyone who remembers it, which is why
it is the only one that needs an anchor.

## How far it has travelled

<!-- claim: lifecycle-stages -->
No stage requires an adjacent one: a proof may declare `covers` without `spec`, `spec` without
`covers`, and `falsified_by` with either or neither.

| Stage | The artifact | Verb | What its absence lets you say |
|---|---|---|---|
| **Claimed** | `<!-- claim: id -->` in prose | — | "the doc says X" — and nothing tracks it |
| **Bound** | `covers = "doc.md#id"` | `owed` → `DANGLING` | "I built X" — and nothing links it |
| **Promised** | `promises = "<reason>"` | `promises` · `burndown` | "I implemented the spec" — never ran it |
| **Proven** | green, with `proves = "<context>"` | `prova` | — |
| **Falsifiable** | `falsified_by = fn` | `falsify` → *vacuous* | "the tests pass" — vacuously |
| **Attested** | executed + passed in the run record | `attest` | "0 failed" — nothing ran |

The middle two stages are one grammar: **`promises` graduates to `proves`** — a tense change,
demanded by a failure the moment the body goes green. (`spec`/`specs`/`--specs`/`--strict-specs` are the deprecated
spellings; they warn and retire at 1.0 — machine field names move in the same release.)

Read the right-hand column as the point. Each row exists because a sentence an agent could say
was, at that stage, unfalsifiable.

## The levels, as a project actually adopts them

<!-- claim: levels-are-independent -->
A package that adopts nothing beyond level 0 pays nothing for the rest: with no `[claims]`
section, no `spec` flag and no `falsified_by`, a run's verdict, output and exit code are exactly
what they would be if none of those capabilities existed.

**Level 0 — Run.** `prova.test(name, fn)`, and `prova`. One manifest key. This must work forever;
everything below is additive.

**Level 1 — Context.** `proves = "why this matters"` on a finished proof. Runtime-inert. The design
story lives next to the assertions it explains, where a reviewer cannot miss it and no separate
doc can drift from it.

**Level 2 — Spec-first.** `promises = "<reason>"` for a contract you can state but are not
building yet. `prova promises` enumerates the open surface; `prova burndown` is the implementing
loop; a promise whose body goes green **fails**, demanding graduation to `proves`. The open
surface becomes executable: `grep TODO` lies, `prova promises` cannot.

**Level 3 — Falsification.** `falsified_by` declares the mutation a body must catch. `prova falsify`
applies it and inverts the verdict. This is the level that distinguishes a proof from a green
line, and it costs nothing on the ordinary path.

**Level 4 — Claims.** `[claims] docs = [...]`, `<!-- claim: id -->` in prose, `covers` on the proof.
`prova owed` reconciles every origin into one list. `--pin` catches the drift where the anchor
survives, the prose is edited, and the proof still passes.

**Level 5 — Attestation.** The run record, and `prova attest <doc.md#id>`. The only level that
asks about a *run* rather than about the source: did the proof for this obligation actually
execute? Everything above it can be satisfied by a suite that never ran.

## The vocabulary problem

The verbs split cleanly into two families, and the naming marks neither.

<!-- claim: two-verb-families -->
A query verb — `promises`, `owed`, `attest`, `evidence` — never executes a proof body. Reading
what a package owes must be safe on any machine, whatever the proofs would do if run.

- **Runs** — execute proofs: `prova`, `prova burndown`, `prova falsify`.
- **Queries** — execute nothing: `prova promises`, `prova owed`, `prova attest`, `prova evidence`.

The runs are already principled: each is sugar over composable primitives (`burndown` is
`--promises --due`, `falsify` is `--falsify --allow-empty`), so the verb is a shorthand and
never a second code path.

The queries were not. `prova promises` lists **nodes** and is a run-with-`--list`; `owed` and
`attest` list **obligations**. Two object types wearing one apparent family. The resolution keeps
every convenient spelling and names the family after its object:

### The decision: the whole account is `prova evidence`

`evidence` sits in the register the rest of the vocabulary already speaks (proof, prove, attest,
claim, vacuous — measured at roughly ten evidentiary words to one of anything else), and it is
already load-bearing in the machine surface as the `no_evidence` attest verdict.

<!-- claim: evidence-is-the-account -->
`prova evidence` reports the whole account — every anchored claim with how far it has travelled,
and the open promised surface — in one command that executes no proof body.

```
prova evidence                  the whole account, stages and debts
prova owed                      the actionable narrowing: only what is owed
prova attest <addr>             one obligation, against the recorded run
prova promises                  a NODE listing, on the run axis (= list --promises)
```

`owed`, `attest`, `promises` and `burndown` all survive as conveniences — the same pattern as
`falsify`: the verb is sugar over a composable primitive, never a second code path.

<!-- claim: ci-can-ask-for-everything -->
`prova attest` with no address reconciles every anchored claim against the recorded run and exits
non-zero unless each one is attested — one exit-code answer for a pipeline.

An address at a time is a developer's question; the pipeline's question is "is everything this
project claims actually evidenced", and it has to be one exit code or CI cannot gate on it.

### The account is a library, not a binary

A CLI is one renderer of the account, and it must not be the only one that can compute it. Today the
reconciliation lives in `prova-cli` — `claims.rs`, `record.rs`, `annotations.rs`, `runstate.rs` — so
anything that is not the `prova` binary is left scraping human prose to learn what a project owes.
`prova-core` already carries the shape of that gap: `ReminderAccount.owed` is documented as *"Supplied
by the caller (the reconciliation lives CLI-side)"*, a hole where the number arrives from somewhere
else. A UI that renders promises going green as a run streams, an editor plugin, and an agent host that
embeds the engine are all the same consumer, and none of them can be served by a `--format json` flag
bolted onto a renderer.

<!-- claim: ledger-is-library-side -->
The obligation ledger — claims, the run record, attestation, and reconciliation — is computed in
`prova-core` behind a typed API that takes paths from its caller and needs no optional feature to
reach, and `prova-cli` is one renderer over it holding no ledger logic of its own.

Two consequences worth stating, because both are easy to get wrong while making the move compile:
the API is **path-injected** (a consumer resolves project roots its own way and must not inherit
prova's `.prova/var` convention to read a record), and it is **not feature-gated** (a ledger a
consumer must opt into is a ledger consumers will not find).

## Prova must be its own exemplar

<!-- claim: prova-dogfoods-its-own-lifecycle -->
Prova's own manifest declares `[claims]` over its design docs, and `prova owed` run against this
repository reports against real anchors — because a lifecycle nobody has run end-to-end on a real
project is a design, not a practice.

Until this doc existed, prova had **zero** claim anchors across forty documents, no `[claims]`
section, and every `covers` binding in the repo lived inside a test sandbox's Lua string. The
claims and attestation halves had never been exercised on a real project, including their own.

Two defects fell out of the first attempt to use them here, which is the argument for dogfooding
in one line:

- `prova owed` and `prova attest` **crashed** on any package with local packages — they resolved a
  package through a thinner slice of manifest resolution than a run uses. Prova's own repo has
  local packages, so both verbs were dead on the flagship project and nobody knew.
- The MCP surface has **no test coverage at all**, so a capability wired to it is unproven by
  construction — the same vacuous green one level out from the suite.

## Decided, and open

- **Naming: decided.** `evidence` over `ledger` (`ledger` is purely financial — outside the
  law-and-evidence register everything else shares) and over `status` (already an MCP tool for
  held topologies, and the most common field in the HTTP driver vocabulary). The attribute is
  `promises` — grammatically parallel with proves/covers/requires, graduating by tense change.
  Status words conjugate from the stages: DANGLING, UNPROVEN, PROMISED, STALE.
- **`attest` vs the supply-chain sense.** in-toto, sigstore and SLSA all use *attestation* to
  mean a cryptographically signed statement. Prova's attestation is deliberately unsigned — the
  threat model is a careless agent, not a malicious one — and the record topic says so plainly.
- **`--since`, parked.** Labelling a failure *introduced* vs *pre-existing* needs either records
  kept per revision (a growing state footprint with a retention policy) or checking out an old
  ref and rebuilding (VCS manipulation, and prova has no concept of a build). Both are more
  invasive than the value is currently proven to be. Revisit when the ledger is in real use.
- **Does level 5 subsume level 2?** An open spec and an unattested claim are both "no evidence
  yet". They are reported by different verbs today. That may be one idea wearing two names.
- **Promises-aware `depends_on`, parked.** Field report from the first production coordination
  checklist: items forming an obvious chain (land → clean → CI → close) deliberately carried no
  dependency edges, because each item is an *observation of an end state* — an unmet upstream
  leaves the downstream red-but-PROMISED, which is exactly the right report ("open", never
  "blocked-unknown"), and sequencing enforced by shared facts (compare against the current head
  sha) beat sequencing enforced by graph edges. If checklist-native gating is ever wanted, the
  right semantics are promises-aware `depends_on`: upstream PROMISED → downstream reports
  PROMISED "waiting on X" — never SKIP. The reporter judged it nice-to-have, not needed; park
  until a checklist that genuinely cannot express its sequencing in data asks for it.
