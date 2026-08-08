# spec — the prose layer, where obligations are written

A spec is what must be true, stated in prose — in a design doc, a README, a ticket. It is exactly
the thing an agent (or a person) can *claim* to have implemented without having done so. prova makes
that checkable by treating the prose as a first-class layer — the spec — and reconciling it against
the proofs. A spec is not a leg you run; it is the medium the legs live in.

## The manifest entry

`[specs]` declares where your spec prose lives — its **sources**. Today a source is directories in
your project:

```toml
[specs]
docs = ["docs/design", "README.md"]
```

**Absent by default**, and absence is the whole point: a package that never opts in scans nothing,
pays nothing, and is never lectured about a subsystem it does not use. Scoped deliberately — prova
never crawls a whole repo looking for prose. (Renamed from `[claims]`, which under-named it: this
section holds more than claims.)

## What a spec contains: two states of one obligation

Within a spec, an obligation is anchored in one of two states — the two sides of the spec coin,
same shape and one shared id namespace, one keyword apart:

```markdown
<!-- backlog: flaky-teardown -->    cold — captured, not yet owed   (prova learn backlog)
<!-- claim: never-preempt -->       owed — reconciled, dischargeable (prova learn claims)
```

A **backlog** item is muted: out of `owed`, off CI, invisible to a run — captured without obligation.
A **claim** is owed until a proof covers it. Promotion flips the keyword in place — the id and prose
do not move — which is only possible because the directory source is read/write. `prova owed`
reconciles the claims; `prova backlog` lists the cold ones.

## Spec, then proof

The spec is the prose side. The executable side is **promise** (a commitment to make a spec real)
and **proof** (a demonstration that it is). A proof crosses back to a claim with
`covers = "doc.md#id"` — the binding `prova attest` and `prova evidence` reconcile. Prose states it;
a proof proves it; the run attests it.

See also: `prova learn backlog` · `prova learn claims` · `prova learn promises`
