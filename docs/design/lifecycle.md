# The obligation lifecycle — the levels of Prova, and the vocabulary for them

> Companion to [`proof-driven-development.md`](proof-driven-development.md). That doc is the
> thesis: done is not a claim, it's a proof that runs. This doc is the *operational* shape of
> that thesis — where an obligation comes from, how far it has travelled, what you write and
> what you run at each level, and what the whole account should be called.

## One line

**Every capability Prova has added answers one question about one obligation: *has this
travelled far enough to be believed?*** The atoms were built one at a time; this doc is the
map they turned out to form.

## Where an obligation comes from

Three origins, and the distinction is not decoration — it decides who can retire it.

| Origin | Written as | Retired by |
|---|---|---|
| **Implicit** — someone wrote a test | `prova.test(name, fn)` | deleting the test |
| **Deferred** — scoped, not yet built | `spec = "<reason>"` | implementing, then graduating the flag |
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
| **Bound** | `covers = "doc.md#id"` | `owed` → `UNBOUND` | "I built X" — and nothing links it |
| **Speced** | `spec = "<reason>"` | `specs` · `burndown` | "I implemented the spec" — never ran it |
| **Proven** | green, with `proves = "<context>"` | `prova` | — |
| **Falsifiable** | `falsified_by = fn` | `falsify` → *vacuous* | "the tests pass" — vacuously |
| **Attested** | executed + passed in the run record | `attest` | "0 failed" — nothing ran |

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

**Level 2 — Spec-first.** `spec = "<reason>"` for a contract you can state but are not building
yet. `prova specs` enumerates the open surface; `prova burndown` is the implementing loop; a spec
whose body goes green **fails**, demanding graduation to `proves`. The backlog becomes executable:
`grep TODO` lies, `prova specs` cannot.

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
A query verb — `specs`, `owed`, `attest` — never executes a proof body. Reading what a package
owes must be safe on any machine, whatever the proofs would do if run.

- **Runs** — execute proofs: `prova`, `prova burndown`, `prova falsify`.
- **Queries** — execute nothing: `prova specs`, `prova owed`, `prova attest`.

The runs are already principled: each is sugar over composable primitives (`burndown` is
`--specs --strict-specs`, `falsify` is `--falsify --allow-empty`), so the verb is a shorthand and
never a second code path.

The queries are not. `prova specs` lists **nodes** and is a run-with-`--list`; `owed` and `attest`
list **obligations**. Two object types wearing one apparent family, with parts of speech drawn
from three different grammars — a plural noun (`specs`), a past participle (`owed`), an imperative
(`attest`).

### The proposal: name the family after its object

<!-- claim: ledger-is-the-account -->
The obligation query family is one object — the ledger — and its verbs must be narrowings of it,
not siblings to it.

```
prova ledger                    every obligation, its origin, and how far it travelled
prova ledger --owed             ≡ prova owed      the actionable debts
prova ledger --attest <addr>    ≡ prova attest    one obligation, against the last run
prova specs                     unchanged — a NODE listing, on the run axis
```

`owed` and `attest` survive as sugar exactly as `burndown` and `falsify` do. What changes is that
there is finally a command that answers "where does this project stand", which today no verb does:
`owed` shows only the debts, and `attest` answers for one address at a time.

<!-- claim: ci-can-ask-for-everything -->
There must be one exit-code answer to "is every stated obligation attested", or CI cannot gate on
the lifecycle at all.

That is the gap `--attest <addr>` leaves: an address at a time is a developer's question, not a
pipeline's.

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

- `prova owed` and `prova attest` **crashed** on any package with local plugins — they resolved a
  package through a thinner slice of manifest resolution than a run uses. Prova's own repo has
  local plugins, so both verbs were dead on the flagship project and nobody knew.
- The MCP surface has **no test coverage at all**, so a capability wired to it is unproven by
  construction — the same vacuous green one level out from the suite.

## Open questions

- **Naming.** `ledger` is the accounting metaphor already used for `owed` ("one ledger over every
  origin"). `status` reads friendlier but collides with `ps`. Decide before it ships, because the
  skill and the topics both name these verbs.
- **`--since`, parked.** Labelling a failure *introduced* vs *pre-existing* needs either records
  kept per revision (a growing state footprint with a retention policy) or checking out an old
  ref and rebuilding (VCS manipulation, and prova has no concept of a build). Both are more
  invasive than the value is currently proven to be. Revisit when the ledger is in real use.
- **Does level 5 subsume level 2?** An open spec and an unattested claim are both "no evidence
  yet". They are reported by different verbs today. That may be one idea wearing two names.
