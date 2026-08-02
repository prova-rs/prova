# evidence — the whole account: where does this project stand?

The doc **claims** it; a proof **promises** it; the implementation **proves** it; the run
**attests** it. Every obligation travels that road — claimed, bound, promised, proven, attested. Each verb below answers one part; `prova evidence` answers all of it at once:

```
$ prova evidence

  CLAIMED     8   anchored claims in the declared docs
  BOUND       6   covered by at least one proof
  PROMISED   35   proofs authored ahead of implementation
  ATTESTED    4   covering proof executed and passed in the recorded run

owed:
  PROMISED    35
  UNPROVEN     2
  (`prova owed` lists each one; `prova attest` gates on the account)
```

A report, never a gate: exit 0, executes no proof body, safe on any machine. The family:

| Verb | Object | Question |
|---|---|---|
| `prova evidence` | the account | where does this project stand? |
| `prova owed` | the debts | what still needs doing? |
| `prova attest <addr>` | one obligation | did the proof for THIS claim actually run? |
| `prova attest` | every claim | is everything we claim evidenced? **The CI gate.** |

These list **obligations** and never execute a proof. `prova promises` looks similar but lists
**nodes** — it is `prova list --promises`, on the run axis.

## The stages, one line each

- **CLAIMED** — `<!-- claim: id -->` in a doc under `[claims] docs`. Prose became an obligation.
- **BOUND** — some proof declares `covers = "doc.md#id"`. The claim has an owner.
- **PROMISED** — flagged `promises = "<reason>"`: authored ahead, red by design, graduates to
  `proves` the moment it goes green (`prova learn promises`).
- **ATTESTED** — the covering proof executed and passed in the recorded run
  (`prova learn record`). Everything above it can be satisfied by a suite that never ran.

## The debts

- **UNPROVEN** — an anchored claim no proof covers. Work someone scoped in prose.
- **DANGLING** — a `covers` naming prose that is not there (not written yet, or deleted).
- **PROMISED** — the open surface: `prova burndown` drives it.
- **STALE** — a pinned claim whose text changed since the pin (`prova owed --pin`).

## Wiring CI

```
prova --profile ci        # the suite holds the line, and writes the run record
prova attest              # then: every claim evidenced, or exit non-zero
```

Bare `attest` fails on any claim that is unbound, or whose covering proof was skipped,
deselected, red, still promised, or absent from the recorded run — "0 failed" is not evidence,
and neither is a proof that never executed. A package with no claims exits 0 and says so.

Note: prova's attestation is deliberately **unsigned** — the threat model is a careless agent,
not a malicious one (unlike in-toto/sigstore attestations, which are signed statements).
