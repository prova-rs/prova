# Verifiers — deputed evidence, and the verdict-ingestion seam

> Companion to [`lifecycle.md`](lifecycle.md) and [`reminders.md`](reminders.md). The lifecycle
> maps how far an obligation has travelled; reminders added the obligations the world creates.
> This doc adds the evidence prova did not gather itself — verdicts produced by other tools,
> conducted and ledgered without ever being trusted on their own say-so. Drafted 2026-08-04;
> the JUnit seam is the first implementation, formal verifiers (TLA+) are the planned second.

## One line

**Prova has two ways to earn a green: witness it, or conduct a specialist and ledger its
verdict.** Both are held to the same lifecycle — covered, attested, recorded — and the second
is what makes prova the quality account of a whole project rather than of one layer of it.

## Two provenances of evidence

<!-- claim: two-provenances -->
Every verdict in prova's account has one of two provenances. **Observed** evidence is what a
proof witnessed itself, at the black-box boundary — the only kind prova has carried until now.
**Deputed** evidence is a verdict produced by another verifier — a unit-test framework, a
linter, a model checker — that a proof conducted and adopted. Prova never crosses the
boundary foundations.md draws ("if the assertion needs to reach inside the process's memory,
it's out of scope"): the *deputy* crosses it, and prova observes the deputy from outside, the
same posture it takes toward every system under test.

This is the boundary-respecting resolution of "prova as THE quality runner": unit tests stay
with JUnit/pytest/cargo, lints stay with clippy/eslint — prova does not reimplement any of
them, it conducts them and accounts for their verdicts. One `prova` then answers "does this
project meet its stated bar?" across every layer that produces a verdict. The dual boundary:
**if it doesn't produce a verdict, it isn't prova's job** — build, format, and deploy belong
to task runners; prova is an evidence ledger that runs things, never a task runner that
happens to test.

## Why JUnit XML is the seam

One format is the de facto lingua franca of test results: the JVM world natively, pytest
(`--junitxml`), Go (gotestsum), Rust (cargo-nextest), Jest/Vitest/Playwright, .NET loggers,
and a long tail of linters and scanners that emit it because every CI system ingests it. One
tolerant parser therefore federates prova's evidence model over essentially the entire
existing testing ecosystem — including prova itself, which already *emits* JUnit XML: one
format, both directions.

The format is under-specified (no official schema, dialect drift), so prova consumes only the
stable core — suite and case names, outcome, message, timing — and tolerates the rest.

## The primitive: `junit`

A **bundled namespace** (reserved, api-freeze §2), not a package: parsing XML requires native
code, and native code is always first-party. Two facets:

```lua
-- The primitive: parse result files (a path or glob) into a structured report.
local report = junit.load("target/surefire-reports/*.xml")
-- report = { total, passed, failed, errors, skipped, files = {...},
--            cases = { { suite, name, outcome, message?, time_ms?, file }... } }

-- The facet: run the tool, ingest its fresh results, adopt the verdict, ledger the cases.
prova.test("the service's own suite holds", { tags = { "unit" } }, function(t)
  junit.verify(t, { run = "mvn -q test", results = "target/surefire-reports/*.xml" })
end)
```

<!-- claim: ingest-structured -->
`junit.load` parses JUnit XML into named cases with outcomes, messages, and timings — never a
blob of stdout. `junit.verify` adopts the verdict by asserting on the report: any failed or
errored case fails the proof with the deputed cases' own names and messages, and **a report
with zero cases fails too** — "the glob matched nothing" reading as green would be the vacuous
pass, one tool further out.

<!-- claim: verify-freshness -->
When `junit.verify` is given `run`, it enforces freshness: result files must be newer than the
moment the command was launched, so a stale artifact from a previous build can never be
adopted as this run's evidence. Given only `results` (the CI-artifact flow, where another step
produced the files), freshness is the caller's stated responsibility — the report carries each
file's path and modification time so the adoption is auditable, but prova does not guess an
acceptable age.

<!-- claim: deputed-not-nodes -->
Deputed cases are never prova nodes. They cannot be selected, re-run, or falsified by prova —
the deputy owns all of that — so they enter the account as their own row type, exactly as
reminders did: a separate kind with separate reporting, never test-shaped impostors in the
tally. The proof that conducted them is the node; its verdict summarizes theirs.

## The record: deputed rows

<!-- claim: deputed-in-record -->
Every ingested case lands in the run record with its provenance: verifier, suite, case name,
outcome, message, and the artifact file it came from. The record's honesty extends one layer
down — "the deputy passed" becomes checkable, case by case, from the same file `attest` and
`evidence` already read.

<!-- claim: attest-deputed -->
`prova attest junit:<suite>#<case>` answers the deputed form of the attestation question: did
this upstream case actually execute and pass in the recorded run? A red, skipped, or absent
case attests nothing — same contract as every other address, one exit code for a pipeline.

## Conducting an expensive deputy — conduct once, read many

<!-- claim: conduct-once-read-many -->
A deputy whose invocation is expensive — `cargo nextest` compiles the workspace before a single
verdict lands — is conducted **once per scope**, never once per claim. A suite- or file-scoped
fixture runs the deputy and returns the artifact path; **one** proof adopts the whole report
(`junit.verify { results }` — the artifact flow, freshness held by construction because the
fixture produced it this run); and any number of sibling proofs `junit.load` the same artifact to
bind one claim to one named case (`covers` on the reader, the assertion on the deputy's own case
name). Claim-granular spec coverage therefore never multiplies compilations: the deputy runs
once, the account adopts every case once, and each additional binding costs a parse. This is also
the v1-compliant answer to "covers binds proofs, not deputed cases": the reader proof *is* the
binding, and living with it is what will inform whether `covers = "junit:…"` ever needs to exist.

Cheap deputies do not need the split — pytest with `--junitxml` conducts fine inside a single
`junit.verify` per proof — so the pattern is per-ecosystem: Rust pays a compilation and gets the
fixture; Python and .NET conduct directly. Choosing the shape is part of instrumenting a project,
and the manifest profile that runs the gate names the choice.

<!-- claim: exclusive-quality-interface -->
**Prova is this repo's exclusive quality interface; build tooling keeps only artifacts.** Every
verdict-producing invocation is a profile — `prova run ut` (nextest deputed via the conduct-once
pattern), `prova run quality` (clippy, the unwrap/expect ratchet, file sizes), `prova run
coverage` (line coverage ratcheted against the committed baseline), `prova run all` (the pre-push
sweep: black-box plus the switched heavy legs) — each carrying a description, so `prova run
--list` and the project card answer "which leg, when". Every verdict lands in the account, so
burndown/backfill/owed see the whole bar. CI's legs are these same profiles, and xtask keeps only
`install`/`build`/`check`/`sweep` plus the `proofs` bootstrap (artifacts, per the two-provenances
boundary: "if it doesn't produce a verdict, it isn't prova's job" — and the converse; the
bootstrap itself retires when #manifest-declared-runner lands). No `it` profile here and rightly
so: cargo's integration-test targets ride the ut conduct, and the black-box suite IS the
system-level integration bar — the split is per-project vocabulary, not doctrine.

<!-- backlog: coverage-of-the-whole-bar -->
**The coverage gate measures the unit layer; the bar is bigger than that.** `prova run coverage`
conducts `cargo llvm-cov nextest`, so its 60% is unit-test line coverage — and the per-file gaps
it reports are misleading at the edges: `modules/socket.rs` reads 2% while owning a whole
black-box proof directory, because proofs drive a separate uninstrumented binary. The next rung:
instrument the proof run itself (build `target/debug/prova` with `-C instrument-coverage`, run
the suite, merge profdata with the nextest set) so the ratcheted number is the WHOLE bar —
observed and deputed evidence landing in one coverage account, the same two-provenances story
the verdicts already tell. Until then, read the unit number as "covered by unit tests", never
"covered". Recorded 2026-08-09.

## The facet convention (for the verifiers that follow)

`junit` is first-party because parsing is native; verifier *packages* (TLA+/TLC over a pinned
image is the planned first) follow the same shape in Lua + Docker: a `verify(t, opts)` facet
that runs the tool, adopts the verdict loudly, and files what it can into the account.

<!-- claim: verifier-falsifiable -->
A verifier facet must be proven able to report red. Its suite carries a negative control — a
fixture the deputy is known to fail — and asserts the facet surfaces that failure; a facet
proven only on green fixtures is a rubber stamp, the vacuity `falsify` exists to hunt one
level further out.

## Boundaries

- **Not a dashboard.** Ingestion serves the obligation ledger — attest, evidence, structured
  failure reports — not result visualization. Allure exists; prova renders counts and rows.
- **No per-case re-run in v1.** The facet knows its tool's selection syntax, so translating
  `--last-failed` into `mvn test -Dtest=...` is a designed extension — deliberately after the
  seam proves out.
- **`covers` binds proofs, not deputed cases, in v1.** A claim is discharged by the proof that
  conducts the deputy; letting a claim bind a deputed address directly (`covers =
  "junit:..."`) is open below.

## Decided, and open

- **Decided: `junit` is bundled and reserved**; `load` is the primitive, `verify` the facet;
  deputed rows are their own record type; `attest` speaks `junit:` addresses.
- **Open: `covers` to deputed addresses** — wants a real consumer before the address grammar
  freezes into the claims scanner.
- **Open: TAP ingestion** — the second cheap format; same rows, different parser. Add when a
  consumer appears.
- **Open: per-case re-run translation** (`--last-failed` → the deputy's own selector).
- **Follow-up phase: formal verifiers.** TLA+/TLC as the first verifier *package*, reusing
  these rows and this contract; trace conformance after that. Tracked on the
  tlaplus-capability checklist.

## Status

- **Drafted 2026-08-04**, implementation landing with it as one proof-carrying change —
  anchors above are covered by `proofs/spec/engine/junit_test.lua`.
