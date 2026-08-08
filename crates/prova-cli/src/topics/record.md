# record — what did NOT run, and attesting an obligation against it

`0 failed` is the last thing an honest agent can say that is true and worthless. It is equally true
of a suite that proved everything and a suite in which every proof **skipped** for want of a docker
daemon, a broker, or a display. The failure count answers "did anything go red". Nobody was asking
that — they were asking "is this covered", and that answer lives in the negative space.

Every run writes a record of it:

```
.prova/var/last-run.json     # always; prova's own gitignored state
prova --record run.json      # ALSO here, for CI to keep as an artifact
```

It carries the counts, and then — individually, not summed — the paths behind the two counts that
mean *no evidence was produced*:

- **skipped** — reached a gate before the body (`requires` unavailable, a failed dependency), with
  the gate's own words for why.
- **deselected** — never in the run at all (`-k`, `--tags`, `--node`, `--promises`, `--falsify`).
  Narrowing a selection is the cheapest way there is to report green having tested nothing.

## attest — the question `owed` cannot answer

`prova owed` is static: it reconciles anchors against `covers` bindings and reports that an
obligation *has* a proof. Whether that proof ever RAN is a fact about a run.

```bash
prova                                          # produces the record
prova attest docs/design.md#drain-not-preemption
prova attest drain-not-preemption              # a unique bare id resolves too
```

```
prova: attest docs/design.md#drain-not-preemption
  ↳ NOT attested — a drain is not a preemption did not execute in the recorded run
    (requires "placement_broker" ("placement_broker" is unavailable))
```

Exit 1 when the obligation is not attested; exit 2 for a usage error — in CI a missing argument is a
broken pipeline, an unattested claim is a real finding.

**Only an executed, passing proof attests.** Skipped, deselected, absent from the record, failed,
still an open `spec`, or covered by nothing at all — every one of those is the absence of evidence.
Exiting 0 on "I found nothing to check" is the vacuous pass this whole line of work refuses. Where
several proofs cover one claim they are all obligations, not a menu: one skipped sibling is enough
to withhold the attestation.

## Reading it directly

The record is plain JSON and stable across runs, so a reviewer or a script can read it without
prova:

```bash
jq '.summary, (.skipped[] | .path)' .prova/var/last-run.json
```

## What it is not

- **Not signed.** The threat model is a careless agent, not a malicious one. An agent that would
  forge a record would equally write a falsifier that mutates nothing; a signature buys key
  management, not truth.
- **Not a gate.** Recording never changes a run's verdict, output or exit code. Machinery that
  taxes the ordinary path is machinery people switch off.
- **Not a coverage metric.** It says what executed, not what was worth executing. Pair it with
  `prova learn falsify` — a proof that ran and cannot fail is its own kind of nothing.

Next: `prova learn claims` (where obligations come from), `prova learn falsify` (whether a proof
that ran is asserting anything).
