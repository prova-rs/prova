# spec — the prose layer, where obligations are written

A spec is what must be true, stated in prose — in a design doc, a README, a ticket. It is exactly
the thing an agent (or a person) can *claim* to have implemented without having done so. prova makes
that checkable by treating the prose as a first-class layer — the spec — and reconciling it against
the proofs. A spec is not a leg you run; it is the medium the legs live in.

## The manifest entry

`[specs]` declares where your spec prose lives — its **sources**. Each source is explicit; today the
one type is a local `directory` (a folder scanned for anchors, or a single markdown file):

```toml
[[specs.source]]
type = "directory"
path = "docs/design"
```

Add a `[[specs.source]]` per source — more than one is fine. **Absent by default**, and absence is
the whole point: no `[specs]`, no scan, and no feature you did not ask for. prova favors
explicitness over an implicit default — capture your conventions in an init archetype
(`prova init project`, `archetect render …`) rather than a magic `docs/`. (`docs = [...]` is the
deprecated flat spelling; it still scans, with a warning.)

## What a spec contains: two states of one obligation

Within a spec, an obligation is anchored in one of two states — the two sides of the spec coin,
same shape and one shared id namespace, one keyword apart:

```markdown
<!-- backlog: flaky-teardown -->    cold — captured, not yet owed   (prova learn backlog)
<!-- claim: never-preempt -->       owed — reconciled, dischargeable (prova learn claims)
```

Either state can carry an optional `YYYY-MM-DD` after the id (`<!-- backlog: id 2026-09-01 -->`) — a
draw-down deadline a reminder can hold you to. A **backlog** item is muted: out of `owed`, off CI,
invisible to a run — captured without obligation. A **claim** is owed until a proof covers it. Promotion flips the keyword in place — the id and prose
do not move — which is only possible because the directory source is read/write. `prova owed`
reconciles the claims; `prova specs --backlog` lists the cold ones.

## Spec, then proof

The spec is the prose side. The executable side is **promise** (a commitment to make a spec real)
and **proof** (a demonstration that it is). A proof crosses back to a claim with
`covers = "doc.md#id"` — the binding `prova attest` and `prova evidence` reconcile. Prose states it;
a proof proves it; the run attests it.

See also: `prova learn project` (this package's writable sources + house rules) ·
`prova learn backlog` · `prova learn claims` · `prova learn promises`
