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

<!-- claim: anchor-records-when-it-was-captured -->
**An anchor carries named properties; `recorded` stamps when the item was captured; deadlines
are composed, not hand-picked.** The positional `YYYY-MM-DD` this grammar once carried could
never say whether it meant recorded-on or due-by — its own author had to ask — and the one
anchor that used it meant *due* while the design intent was *recorded*, so the slot was retired
outright rather than reinterpreted: a bare date after the id is a taught error naming both named
spellings. The grammar now: `<!-- backlog: id key=value … -->`. Two keys are **blessed** — prova
itself relies on them, both ISO-validated, both optional: `recorded=` (when the item was first
written down; the MCP `capture` tool stamps it automatically, making it the ideal on every
anchor) and `due=` (a hard external deadline — the deprecation bridges' "gone by 2027-01-01", a
commitment rather than a function of age). Every other key is the author's own vocabulary,
parsed and passed through verbatim. The reminder surface `account.specs` carries every anchor as
`{ address, kind, recorded?, due?, props }`, so draw-down is whatever a `when` composes:
`date.days_since(o.recorded) > 30` is the sliding window every captured item gets for free —
the lane slides all its deadlines by changing one number in one reminder — and `date.past(o.due)`
draws down the commitments; the mirror policy on the owed side ("no claim has sat unproven for
more than N months") is the same one-liner against `kind`. Properties belong to the **anchor**,
not to the cold state: promotion flips the keyword and touches nothing else, so `recorded` keeps
meaning what it says — when this was first specified, which on a claim is precisely the fact
worth keeping. Blessed keys may grow over time; they never become required. The `--undated`
nudge means "captured before the stamp existed"; the pass that stamps `recorded` onto existing
anchors across repos is a prova-driven checklist of its own.

<!-- claim: found-work-is-captured-not-absorbed -->
**A bug found while working a promise — and not covered by it — is recorded as backlog, claim, or
promise before the work moves on; the taught loop says so.** The table above names three origins,
but the commonest in practice is a fourth: *discovery mid-work* — driving one promise green
surfaces a defect, a gap, or a wrong assumption the promise does not cover. Without the teaching,
a discovery has exactly two informal exits: scope-creep into the current change (the fix rides
along, unstated and unproven at its own bar) or a mental note (the one ledger that never survives
the session); both lose the fact. So every surface that teaches the loop carries the sentence —
the skill's step 4 routes what green surfaces to its capture block, and `prova learn pdd`'s step 4
names the three lanes in place: a backlog anchor when it is a shelf item, a claim when it is
specified but unproven, a promise when the proof can be written today (the red-by-design body *is*
the record). Choosing among the three is nothing new to learn — it is just how far the discovery
has already travelled (the ladder above). Session evidence, the day this was recorded (2026-08-10):
shipping the first registry-resolved archetype surfaced three prova defects; each was
fixed-with-proof on the spot, but only because the finder happened to hold the context to — a
finder without it had no taught place to put them down. Capture *mechanics* are the sibling item
(`docs/design/mcp-mode.md#backlog-capture-is-a-taught-procedure` — where an item goes so the
ledger scans it); this one is *when* the loop demands one exists at all.

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

## Field reports (Substrate gate-integration run, 2026-08-11)

<!-- claim: fixture-failure-memoization recorded=2026-08-11 -->
**A fixture's factory runs at most once per scope instance, whatever the outcome.** Success
was always memoized; failure now is too: the first error is recorded on the scope, and every
later consumer in that scope instance is poisoned with the one recorded error — named as a
memoized replay, never a fresh attempt. Nothing changes between attempts at file scope except
the clock, so re-provisioning a failed conduct has no upside and multiplies its cost by the
reader count. The poison lives exactly as long as a cached success would: a `Scope.Test`
fixture still provisions per test, because each test is a fresh scope instance. (Field report
2026-08-11: a `Scope.File` nextest conduct hit its timeout and five dependent readers re-paid
5 × 600s of cargo before the run ended.)

<!-- backlog: resumable-runs-incremental-verdicts -->
**A failed sweep re-pays the whole world; runs should resume and verdicts should cache
against their inputs.** A ~45-minute `run all` that dies at minute 40 (contention, timeout,
one ratchet regression) costs another 45 to re-ask questions whose answers did not change.
Two composable remedies: (a) run journaling + `--resume` — re-execute only nodes that were
red, unattested, or absent last run, in the same tree state; (b) verdict caching keyed on
declared inputs (the conduct command + the tree paths a proof reads — the fixture/deputy
already names both), so an unchanged proof over unchanged inputs replays its verdict and
says so. The tally must distinguish replayed from re-earned — a cached green that reads
identically to a fresh one would green-wash. Observed 2026-08-11: four sweep attempts to
land one slice; three re-paid the ut conduct for reasons unrelated to it.

## Falsification is only sound if the mutant is bounded

<!-- claim: falsify-bounds-a-hanging-mutant recorded=2026-09-09 -->
**Under `falsify` every test is bounded, and exceeding the bound is `VACUOUS-HANG` — its own
verdict, never inverted.** A falsified test that HANGS instead of failing had no bound at all:
`falsify` applies the mutation and inverts the verdict, but a mutation that turns a terminating
loop into a spin (a watcher whose end condition is removed, a retry whose exit flag is stubbed)
never returns a verdict, and the run hangs with it. Witness 2026-09-09 (Substrate,
`proofs/recipes/sdlc_watched_leg.prova.lua`): falsifying the grace check of a slot watcher made its
fixture poll forever; the author added a 50-poll spin guard to the fixture to turn the hang into a
red, and reached for a `--timeout` flag that does not exist.

**This is a soundness bug, not a missing convenience, and that is what decides the defaults.**
`invert_for_falsify` is total over `{Passed, Failed, Skipped}`, but the outcome space of running a
body is `{Passed, Failed, Skipped, ⊥}` — and ⊥ has no inverse. Worse, the mutation class falsify
most exists to catch, *delete a terminating condition*, is precisely the class that produces ⊥. A
mutation engine must assume the mutant is hostile to termination. So the bound cannot be something
an author opts into: a precondition for a verb's correctness that the author has to remember is not
a precondition. `item.timeout` is `Option` and defaults to `None`, and agents — the primary
authors — do not know to declare it.

**`VACUOUS-HANG` is named as kin to *vacuous* on purpose.** Both mean *this falsifier told you
nothing*: one because the body survived the mutation, one because the body never answered. The
tally reads `1 vacuous-hang, 0 vacuous-pass` and the family is obvious at a glance — which matters
more for an agent than a human, because an agent pattern-matches the verdict token and cannot see
that a terminal has stopped moving.

**A hang must never invert.** Today a declared `timeout` under falsify happens to reach the right
answer by the wrong road: the timeout arm in `run_one` returns early and so never reaches
`invert_for_falsify`. Nothing documents that and nothing covers it — so a refactor folding the
timeout into the ordinary result path would silently turn every hang into a falsification
*success*, the strongest possible green meaning nothing. The non-inversion is now stated and
proven rather than incidental.

**Three bounds, one vocabulary already in the tree.** `shell.run` long ago worked out the honest
frame — `timeout` (the wall-clock outer bound, prices the whole task), `idle_timeout` (the liveness
bound: no output and no CPU progress for a window — *bounds death, never work*), `first_byte` (the
start-up bound) — and the same words now mean the same things at test scope. That is the whole
ergonomic: one concept to learn, not two, and an author who knows `shell.run` already knows this.
The progress signal at test scope is what the test itself emits — **assertions and output** — which
is the test-layer analogue of bytes-on-a-pipe.

- **`timeout`** stays exactly what it was: declared per test, the outer bound, honored everywhere.
- **`idle_timeout`** is new at test scope, declarable per test, and useful on the ordinary path:
  it is the bound that distinguishes a wedged body from a slow one, so it can be generous without
  being useless.
- **Under `falsify`, `idle_timeout` defaults on** — and only there. The unmutated body is known to
  assert (it passed, or the pass would not be worth falsifying), so *no assertion and no output for
  a window* is a sound wedge signal for the mutated run specifically. Defaulting it on the ordinary
  path would be a different and much larger claim, and is not made here.
- **`--timeout <dur>`** is the run-scoped override: it caps every test in the run, overriding what
  each declares. Not falsify-specific — it is the debug-run affordance the witness reached for, and
  it composes with any selection.

**What this does NOT bound is a Lua loop that never awaits.** All of the above ride
`tokio::time::timeout`, which needs the body to yield. `while true do end` yields nothing and is
unbounded still; that needs the `mlua` interrupt hook that
[architecture.md](architecture.md#timeouts-the-three-mechanisms) lists as mechanism 2, still
planned, and it is captured as `agent-ergonomics.md#pure-lua-case-liveness` rather than smuggled in
here. The witness case is bounded by this change because a *polling* fixture awaits every round;
saying so precisely is the difference between a fix and a claim of one.
