# verifiers — deputed evidence: conduct a specialist, ledger its verdict

Prova has **two ways to earn a green: witness it, or conduct a specialist and ledger its
verdict.** Observed evidence is what a proof saw itself at the black-box boundary. **Deputed**
evidence is a verdict another verifier produced — a unit-test framework, a linter, a model
checker — that a proof conducted and adopted. Prova never reaches inside the process; the
*deputy* does, and prova observes the deputy from outside, exactly as it observes any system
under test. Unit tests stay with JUnit/pytest/cargo; prova doesn't reimplement them, it
conducts them — which is what makes one `prova` the quality account of a whole project.
(Dual boundary: if it doesn't produce a verdict, it isn't prova's job — build/format/deploy
belong to task runners.)

## junit — the seam

JUnit XML is the de facto lingua franca of results (JVM natively, pytest `--junitxml`,
gotestsum, cargo-nextest, Jest/Playwright, .NET loggers, many linters), so one tolerant parser
federates prova over essentially every framework:

```lua
prova.test("the service's own suite holds", { tags = { "unit" } }, function(t)
  junit.verify(t, { run = "mvn -q test", results = "target/surefire-reports/*.xml" })
end)
```

- `junit.verify` runs the deputy **tolerantly** (its exit code is not the verdict — its
  results are), enforces **freshness** in run mode (stale artifacts are never this run's
  evidence), refuses **vacuity** (zero parsed cases fails — a wrong glob must not read green),
  and fails with the deputed cases' own names and messages.
- `junit.load(pattern)` is the primitive: parse without running or ingesting — for probing
  with `prova eval`, or CI-artifact flows where another step produced the XML.
- Deputed cases are **never nodes**: the deputy owns selection/re-runs/falsification; the
  conducting proof is the node. Cases land in the run record with provenance, and
  `prova attest junit:<suite>#<case>` answers "did this upstream case actually run and pass?"
  with one exit code.

## Lanes — one `prova` for every layer

Tag the conducting proofs and shape lanes in the manifest; `prova run --list` shows them:

```toml
[profiles.ut]
description = "fast unit lane"
tags = ["unit", "!slow"]     # the lane's set; CLI selection narrows WITHIN it
```

`prova` is the whole bar — proofs, deputed unit suites, lints; `prova run ut` is the inner
loop. A verifier facet's suite must include a **negative control** (a fixture the deputy
fails) — a facet proven only on green fixtures is a rubber stamp.

Formal verifiers (TLA+/TLC over a pinned image) are the planned next deputy, as a package
following the same `verify` facet. See docs/design/verifiers.md for the contract.
