# Reminders — obligations the world creates

> Companion to [`lifecycle.md`](lifecycle.md). That doc maps how far an obligation has
> travelled once it is *stated*; this one adds the obligations nobody can state as work yet,
> because the world has not created them — and the mechanism by which they arrive on the
> world's schedule instead of being remembered on ours. Drafted 2026-08-04, from the
> coordination-checklist work (the ephemeral checklist archetype and its terminal item).

## One line

**A proof guards the past; a reminder watches the future.** `prova.remind` states an
obligation whose *trigger* is a condition of the world and whose *discharge* is an act —
project management, not specification. The grammar extends by one clause: the doc claims it;
a proof promises it; the implementation proves it; the run attests it; **the world remands
it**.

## The sentence that was unfalsifiable

Every lifecycle stage exists because a sentence an agent could say was, at that stage,
unfalsifiable. The sentence here:

> "Nothing needs our attention."

Today nothing tracks it. A project's currency with the world — an upstream that released the
version we are waiting on, a dependency that moved past our pin, a sibling service that
shipped the contract we integrate against, a checklist whose last real item just graduated —
lives in someone's memory, a calendar, or a TODO. All three are the unfalsifiable-prose
failure modes prova exists to kill, and `prova owed` cannot answer for them because the
ledger has no object whose job is *attention*.

The discovery name for this was the **reverse checklist**: a forward checklist item is work
you can start now; a reverse item is work that does not exist yet and *arrives* when the
world moves. Both belong in the same account.

## Why promises cannot carry it

The near-miss encoding is seductive and was found immediately: flag a test `promises` and
write the *trigger* as its body. Dormant while the world holds still (body red → PROMISED,
quiet); the moment the world moves, the body goes green and the graduation demand fires —
a working, mechanism-strength alarm. It even feels clever. It is wrong twice:

1. **It breaks burndown mechanically.** `--due` means *open promises fall due as real
   failures* — that is the implementing loop's contract. Under this encoding a dormant
   reminder is an open promise, so a suite with three release-watchers can never run
   `prova burndown` clean: the agent is handed "upstream has not released v1 yet" as work to
   implement, which is not work, not theirs, and not implementable. Two meanings of "open
   promise" — *spec awaiting implementation* and *tripwire awaiting the world* — cannot share
   one flag.
2. **The vocabulary lies.** When the trigger fires, prova demands graduation to `proves` —
   but "proving" that v1 exists is a vacuous durable assertion nobody wants. The true
   response is *act, then silence the reminder*. A report that describes an alarm as a
   bookkeeping error is off-register for a tool whose reports are the product.

The other encoding — a plain test that fails when the world moves — is worse: it pollutes
FAIL. CI goes red on a change that broke nothing, which teaches people to ignore red; that is
the disease, not a cure.

<!-- claim: reminders-are-not-promises -->
A promise is a specification: its body is the definition of done, burndown implements it, and
it retires by graduation. A reminder is not: its condition is a trigger, its message is an
instruction, its discharge is an act outside the suite, and nothing about it graduates. The
two are separate constructs with separate reporting, and neither is expressible as the other.

## The primitive

```lua
prova.remind("stay current with prova-redis", {
  when = function()
    local latest = releases.latest("https://github.com/prova-rs/prova-redis")  -- a recipe over shell/git
    local pinned = "v1"                                                        -- or read from the manifest
    -- Falsy = watching. Truthy = due; a string becomes the report's "why".
    return semver.gt(latest, pinned) and (latest .. " is out; we pin " .. pinned)
  end,
}, "integrate the new release and bump [dependencies]")
```

- **`when`** is plain Lua over the existing primitives (`shell`, `fs`, `http`, recipes) — no
  condition language, ever. Returning a string instead of `true` supplies the *why*, so the
  fired report says what the world did (`v1.3 is out; we pin v1.1`), in the message-is-the-
  interface tradition.
- **The message is the instruction.** Because the discharge is an act, the thing a reminder
  carries is what to *do*, not what to assert.
- **One-shot and standing forms fall out of one rule: a reminder is due whenever its
  condition holds.** The standing form (above) compares two observables and silences itself
  when you act — bump the pin, back to watching; it is policy, not an event. The one-shot form
  ("v1 exists") holds forever once true; acting on it ends with deleting the reminder or
  rewriting it into the standing form. No latching, no event log, no state beyond the run
  record: v1 evaluates the condition fresh each run, and what it said last is what the record
  holds.

<!-- claim: attention-not-implementation -->
A due reminder demands attention, not implementation: burndown never selects reminders, an
open reminder never fails a run under `--due`, and no verb ever asks an agent to "make a
reminder pass". The sanctioned agent behavior on DUE is to surface it to the human (or act
only when the message itself directs an act the agent may take).

### Looks like a test, is not one

Reminders are declared in proof files, beside the tests whose world they watch, and are
collected by the same machinery. This is precedent, not exception: fixtures and topologies
already share the declaration surface without being tests. The *declaration surface* is
shared; the *account* is separate — a reminder never appears in the test tally, never emits a
PASS line, and a watching reminder is silence in the run output.

## Two accounts, strictly separated

<!-- claim: two-accounts -->
The run headline remains the evidence account of the system — `N passed, M failed, K
promised` — and reminders never contribute to it. Fired reminders add their own line
(`2 reminders due`) and their own report section; a suite whose only motion is the world's
reports the same pass/fail/promised it reported yesterday.

The separation is the point. The evidence account answers "is the system correct?" and gates
merges. The attention account answers "does this project owe the world anything?" and gates
nothing by default — it is read by humans, agents, and dashboards. Conflate them and each
destroys the other's signal: world-motion blocks unrelated merges, or regressions hide in a
wall of nags.

### Outcomes

| state | meaning | where it appears |
|---|---|---|
| **WATCHING** | condition evaluated false — armed, the world holds still | `prova reminders`, `evidence` |
| **DUE** | condition evaluated true — attention owed | run's reminder line, `reminders`, `owed`, `evidence` |
| **UNEVALUATED** | condition could not run (capability absent, error, or no recorded run) | `reminders`, `evidence`, with the reason |

<!-- claim: due-is-not-failure -->
DUE is non-fatal by default: a plain `prova` run with due reminders and green proofs exits 0.
The world moving is not a defect in the change under test. A pipeline whose *job* is currency
opts in — `heed = true` (`[run]` or a profile) promotes it, the exact `must_run` pattern: a
laptop stays friendly, the lane that guarantees attention fails loud.

<!-- claim: unevaluated-never-watching -->
UNEVALUATED must never impersonate WATCHING. A tripwire that could not look is not a tripwire
that saw nothing: a condition gated on an absent capability, or one that raised, reports as
unevaluated with its reason — a disarmed watcher stays visibly disarmed.

## Evaluation and the query family

<!-- claim: conditions-evaluate-in-runs -->
Conditions evaluate during **runs**, in a phase after the proofs complete, and the results
land in the run record. Query verbs (`reminders`, `owed`, `evidence`) execute nothing — they
read the record. A reminder that checks GitHub releases must not make `prova owed` a network
call; the two-verb-families invariant holds exactly as it does for everything else.

Running after the proof phase is load-bearing, not incidental — it is what makes ledger
conditions (next section) coherent: by the time reminders evaluate, this run's verdicts,
promises, and attestations are known.

<!-- claim: reminders-verb-exit-contract -->
`prova reminders` lists every reminder with its state, why, and message, and exits non-zero
if any is DUE — the `attest` pattern: the pipeline's question ("is anything owed attention?")
gets one exit-code answer. `prova owed` includes DUE reminders in its narrowing — an arriving
agent asks one question, and attention owed is part of the answer — while WATCHING and
UNEVALUATED appear only in `reminders` and `evidence`.

The naming follows the vocabulary decision `lifecycle.md` already made — name the query after
its object: `promises` lists nodes, `owed` lists obligations, `reminders` lists reminders.
(`--due` is untouched and unrelated despite the rhyme: it makes *promises* fall due by
decree; a reminder falls due by the world. Both mean "the time is now", which is consonance,
not collision. **WATCHING** replaced the draft's QUIET — quiet named the *output behavior*
rather than the state, and misread as muted/snoozed; watching says what the reminder is doing
and makes the armed/disarmed pair against UNEVALUATED legible at a glance. Its consonance with
`prova watch` is the `--due` kind: both mean "respond when it changes".)

### Cadence, honestly

<!-- claim: no-daemon -->
Prova is a runner, not a daemon, and reminders do not change that. "Whenever the world
moves" means **at every evaluation**: conditions run when runs run. Locally that is constant;
on the wall clock it is one line of CI cron. Prova states and reports the obligation; the
scheduler is whatever already schedules runs. Growing a daemon, a notification transport, or
a polling loop to compete with cron is a non-goal in perpetuity — the report is the product,
delivery belongs to the things that already deliver.

## Conditions over the ledger itself

The condition receives a read-only view of **this run's** account, so a reminder can watch
the suite it lives in:

```lua
prova.remind("this checklist has served its purpose", {
  when = function(account)
    return account.owed == 0 and account.failed == 0
  end,
}, "fold what was learned into the projects' docs, then delete this directory — from outside")
```

<!-- claim: ledger-conditions -->
The account view exposes this run's counts (`passed`, `failed`, `skipped`, `promised`) and
the obligation ledger's remainder (`owed`), evaluated after the proof phase — so "all proofs
green", "all promises fulfilled", and "nothing owed" are one-line conditions. This is what
makes the checklist archetype's terminal item a reminder rather than a hijacked promise:
watching while work remains, DUE exactly once, when deletion is the only thing left.

<!-- claim: no-reminder-fixpoint -->
Reminders cannot observe reminders. They evaluate in one pass, after proofs, in declaration
order, and the account view carries no reminder state — otherwise ordering becomes semantics
and the account needs a fixpoint. One pass, one direction: evidence first, attention second.

Address-level predicates (`account:attested("PLAN.md#phase-1")`) are designed-for but not
v1-committed — see open questions. They are the phase-gating story: a *set* of obligations
opening when a prior stage's claims attest.

## What stays a pattern (and what this unlocks)

The primitive is deliberately one construct with one rule. Everything recognizable is a
composition, taught by `learn` and rendered by archetypes, never grown into core:

- **The terminal checklist item** — a ledger-condition reminder; the checklist archetype's
  always-PROMISED convention upgrades to it on landing.
- **The reverse checklist** — reminders pointed at the world: releases, tags, deprecation
  dates, cert expiry, an API sunset. The checklist pattern gains its intake half: items that
  *arrive*.
- **Dependency currency** — the standing form over a manifest: "are all of this project's
  dependencies, in whatever form, current?" Ecosystem packages ship the observables
  (`cargo.outdated()`, `npm.outdated()`, a `releases.latest` recipe); the reminder composes
  them. Prova states the policy and reports the drift — it never becomes the bot that opens
  the bump PR. Dependabot acts; prova *accounts*.
- **Fleet / SOA currency** — each service carries reminders on its upstreams; one service
  bumping makes the dependents' next scheduled runs report DUE, so "what does the fleet owe
  right now?" is `prova reminders` per repo, aggregated. An architecture's integration state
  becomes a set of small state machines driven by conditions — visible, executable, and
  never in anyone's head.
- **The heeding lane** — a scheduled run with `heed = true` whose only job is currency: its
  red is an *intake signal*, not a defect. The lane hands the DUE report to an agent, the
  agent opens the PR that discharges the instruction, and the reminder goes back to watching
  on merge. This is the SDLC state machine composing: `heed` controls where a state must be
  fulfilled, while the default stays advisory — users assemble their own workflows from the
  same two knobs.

## Boundaries

- **No daemon, no notifications, no acting** (claims above). Report, don't deliver; state,
  don't remediate.
- **Conditions should be cheap and are capability-gated** like everything else
  (`requires = { "network" }` on the reminder skips evaluation → UNEVALUATED, visibly).
- **Not an event system.** A reminder has no memory of past firings; "fired then handled
  then re-fired" is just the condition's value over time, readable from run records. If real
  event semantics are ever wanted, that is a different design and should say so.

## Decided, and open

- **Decided: separate construct** (`prova.remind`), never a test flag — the promise flag
  stays pure, per the failure analysis above.
- **Decided: `when` is Lua**, truthy-string-as-why; the message is the instruction.
- **Decided: reporting split** — WATCHING/DUE/UNEVALUATED, non-fatal default, burndown
  exclusion, `reminders` verb with the attest-style exit contract, DUE in `owed`.
- **Decided: the promotion knob is `heed = true`** (`[run]` or a profile) — one word, the verb
  for exactly this. Like `must_run` it is a guarantee and can only tighten: `[run] heed` OR the
  profile's, so a laxer profile can never silence a promised bar.
- **Open: the account view's exact surface.** Counts + `owed` are committed; address-level
  predicates (`:attested(addr)`, `:retired(addr)`) are the phase-gating extension and want a
  real use case (the tlaplus coordination checklist is the candidate) before the surface
  freezes.
- **Open: `covers` on a reminder.** A checklist's PLAN.md "Exit" anchor describes an act, not
  evidence — can a reminder bind it, so `owed` ties the deletion obligation to prose? Leaning
  yes, but it makes an anchor dischargeable by something that never produces a proof; decide
  with the archetype upgrade in hand.
- **Open: MCP + JSON.** A `reminders` MCP tool mirroring the verb; reporter fields for the
  three states. Should land with v1, shapes TBD alongside the run-record change.

## Status

- **Drafted and implemented 2026-08-04**, as one proof-carrying change: every anchor above is
  covered by `proofs/spec/engine/reminders_test.lua`, which drove the implementation. Shipped:
  `prova.remind` (collected beside tests, never a node), the post-proof evaluation pass with
  the account view, WATCHING/DUE/UNEVALUATED in the run record (`#[serde(default)]`, so old
  records parse), the run's attention section (console only; JSON/TAP untouched), `heed` in
  `[run]`/profiles, the `prova reminders` verb, DUE in `owed`, counts in `evidence`, the
  LuaCATS stub, and `prova learn reminders`. Filtered runs (`-k`, `--promises`, `--falsify`)
  do not re-evaluate — a partial account would fire ledger conditions early — and carry the
  previous record's rows forward.
- Still open, beyond the questions above: MCP surface, JSON reporter events (the record
  carries the account meanwhile), and the checklist archetype's terminal item upgrading from
  always-PROMISED to a ledger-condition reminder — then the tlaplus coordination checklist as
  the first real consumer (items deputing to sibling suites, its terminal item a reminder,
  its upstream watch a reverse item).
