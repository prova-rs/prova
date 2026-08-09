# claims — obligations from prose, and what is owed

Specs do not only come from prova. They come from design docs, READMEs and tickets — and an agent
can say it implemented one without ever having done so. Claims make that checkable.

A `<!-- claim: id -->` anchor is a deliberate act: *this sentence is normative and I intend it to
be proven*. It renders as nothing, so prose carrying a machine-readable obligation still reads as
prose. **Unanchored prose stays invisible** — most prose is reasoning, not contract, and inferring
claims from unmarked text is how this pattern turns into ritual and gets routed around.

```markdown
<!-- claim: busy-not-absent -->
Contention and absence are different answers and must never be conflated.
```

```lua
prova.test("a saturated pool is never reported as unsatisfiable", {
  covers = "docs/design.md#busy-not-absent",
}, function(t) … end)
```

```bash
prova owed          # the ledger: every obligation, from every origin
```

```
DANGLING  docs/design.md#not-written-yet
          contract_test › … covers it, but no anchor exists — write the prose, or
          retire the reference into `proves`
UNPROVEN  docs/design.md#never-preempt      docs/design.md:6 — no proof covers it
PROMISED  contract_test › leases expire     not built yet
```

Open specs and unproven claims share one list on purpose: an agent orienting in a repo asks ONE
question — *what is owed here?* — and an answer living in two places has one that goes stale.
Origin is a column, not a separate concept.

## Opting in

Declare a spec source under `[specs]` — `[[specs.source]] type = "directory"` (`prova learn spec`).
**Absent by default**: no section
means no scan, no cost, and no lecture about a subsystem the package never asked for. Scanning
belongs to the verb — `prova` does not parse markdown to run a test, and an unproven claim never
turns a green suite red.

## The outcomes

Reported, never fatal — with one exception.

- **DANGLING** — a `covers` naming no anchor. Two situations produce this identical state: prose
  not written yet, and prose deleted once the proof captured the contract. Both are unfinished
  work. For the second, retire the claim's context into `proves` — deleting a design doc should be
  lossless, exactly the way a spec's reason graduates.
- **UNPROVEN** — an anchored claim nothing covers. The intake half.
- **STALE** — a *pinned* claim whose text changed (below).
- **Duplicate id in one file** — an **error**. Unlike the rest this is a defect: an ambiguous
  address cannot be discharged by anything.

External addresses (`jira:PROVA-142`) are opaque here — unresolvable is not unbound, and saying
otherwise sends an agent hunting for prose that was never local.

## Pinning — catching drift that stays green

The nastiest drift keeps everything green: the anchor survives, its prose is edited, the proof
still passes — and now discharges a claim it no longer matches. Every id still resolves, so
nothing above sees it.

A pin records the claim's text: `covers = "docs/design.md#busy-not-absent@8c211392"`, written by
`prova owed --pin`. An edit is then reported STALE until re-confirmed.

**Opt-in per binding**, so you pin the claims whose exact wording is the contract and leave the
rest loose. Whitespace is normalized before hashing — a pin that fired when someone reflowed a
paragraph would be switched off within a week — but case and punctuation count, because "must"
and "may" are the whole content of a normative claim.

The pin lives in the proof source rather than a lockfile: in a diff it reads as *"this claim's
text changed and someone re-accepted it,"* which is the signal a reviewer wants.

## Not ready to owe it? — backlog

A claim is owed the moment you anchor it. When you want to capture something *without* owing it yet
— a bug found mid-task, a spec section worth shaping but not now — anchor it `<!-- backlog: id -->`
instead. Same shape, same id namespace, one keyword apart: a backlog item is the muted, cold state
of a claim, out of `owed` entirely until you `prova specs promote` it. See `prova learn backlog`.

## The other direction — backfill

The mirror of `owed` (claim → proof) runs proof → claim: `prova specs backfill` lists every proof no
claim backs (empty `covers`) and gates — a red→green worklist for full spec-coverage. It names the
proof; you anchor the claim that means something (never auto-stubbed — that would be vacuous prose).
