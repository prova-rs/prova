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

<!-- claim: coverage-of-the-whole-bar -->
**Coverage measures the whole bar, layered: one conduct, three ratcheted numbers.** `prova run
coverage` conducts the unit layer (`cargo llvm-cov nextest`) and the black-box layer (the proof
suite through an instrumented prova, every `prova.bin` child writing its own profraw), reporting
each alone AND merged: `rust.coverage.unit`, `rust.coverage.blackbox`, `rust.coverage.lines`.
The layers earn their separation — on landing day they overlapped barely two-thirds (unit 60.2%,
black-box 61.4%, merged 81.2%), and the DELTA is the signal: the conduct prints the unit-owed
worklist (files rich in black-box coverage but naked at the unit layer — proven behavior with no
fast local feedback), which converted a misleading "socket.rs: 2%" panic row into an informed
"black-box 82%, unit 2%" choice. Distinctness is guarded, not assumed: identical layer totals
fail the gate outright, because three conducts read unit == blackbox == merged to fourteen
digits — cached profdata, then a mis-nested target dir, then profraws written where `report`
never looks — before the guard existed. Expense is managed by construction: builds isolate into
llvm-cov's own target dir and only measurement DATA is cleaned between conducts (builds stay
incremental), the class is switch-gated out of every sweep, and CI gives it a nightly leg rather
than a per-push one.

<!-- claim: baseline-bank-policy -->
**Baselines hold by default and ratchet by declaration — `goal` is the intent marker.** Bare
`--update-baseline` establishes first-sights (a metric with no floor gates nothing) and tightens
ONLY goal-carrying metrics (active debt); a goal-less metric is a protection whose committed
floor never moves without a hand — its improvements stay green and unbanked (steady-state slack
is a feature; the report names each held metric with the deliberate-banking spelling). **Named
banking** (`--update-baseline=<name,…>`, the `--heed=SEL` spelling family) moves exactly the
matching metrics, goal or no goal, and a selector matching nothing recorded is a loud refusal —
a typo never reads as a successful bank. **`tolerance`** per metric absorbs measurement noise:
red only when worse than floor − tolerance, a reviewed number in the committed file, never a
loosened floor. The refuse-to-loosen guard is absolute on every flag path; deliberate loosening
is a hand edit reviewed in the PR diff, and the ratchet failure message says exactly that.
(The pre-policy behavior — every improved metric tightened on every bank — was over-ratcheting,
measured live on this repo's own coverage floors, 2026-08-09→11.)

## The facet convention (for the verifiers that follow)

`junit` is first-party because parsing is native; verifier *packages* (TLA+/TLC over a pinned
image is the planned first) follow the same shape in Lua + Docker: a `verify(t, opts)` facet
that runs the tool, adopts the verdict loudly, and files what it can into the account.

<!-- claim: verifier-falsifiable -->
A verifier facet must be proven able to report red. Its suite carries a negative control — a
fixture the deputy is known to fail — and asserts the facet surfaces that failure; a facet
proven only on green fixtures is a rubber stamp, the vacuity `falsify` exists to hunt one
level further out.

<!-- claim: reports-are-custody-not-visualization -->
A deputed conduct hands back **three** things, and prova adopted two. Cases go to the ledger,
measurements go to the ratchets, and the deputy's own artifact — llvm-cov's HTML, its per-file JSON,
a junit file — was dropped: it landed under `target/`, which the sweep deletes, and nothing named
it. So `prova run coverage` could refuse a regression at 73.46% and be unable to show which lines
moved, having computed that answer and discarded it. Diagnosing that layer cost days for want of a
file prova had already made.

`report.publish{ name, summary, forms, explains? }` closes it. The artifact is **copied** into
`.prova/var/reports/<name>/` at publish time — not referenced, because `target/` is swept and a
fixture's tempdir is reaped, and a recorded path that rots is worse than no report, since it reads
as available. Publishing is the moment the file is certain to exist.

**This is custody, not the dashboard the boundary below forbids, and the distinction is load-bearing:
prova never renders an artifact.** The deputy already did. Prova preserves it, gives it a stable
address, and renders one line — the required `summary`, counts and rows, exactly what it rendered
before. A report with no summary is refused, because a file path with extra steps helps nobody.

`forms` is a list of renderings of one fact — `{ json = …, html = … }` — because the two readers
differ and neither should cope with the other's format. An agent takes the JSON, a person opens the
HTML, from a single publish. That is what makes the surface equally useful to both, and it is why
forms are enumerated rather than fixed as a human/machine pair: lcov, TAP, a text summary need no
new vocabulary.

`explains` names the measurements the artifact is evidence for, so a red ratchet can point at where
the explanation lives instead of leaving a reader to rebuild the conduct — the ergonomic the whole
feature exists for.

Read it two ways, because discovery and addressing are different needs: `prova reports` LISTS what
exists with each summary and its forms; `prova reports <name> --kind html` prints that path ALONE,
so `open $(prova reports coverage --kind html)` is the whole viewing story and no platform-specific
opener has to live in prova. The run record carries the same rows, so agents and `--format json` get
it without parsing a console.

## Boundaries

- **Not a dashboard.** Ingestion serves the obligation ledger — attest, evidence, structured
  failure reports — not result visualization. Allure exists; prova renders counts and rows.
  **Custody is not an exception to this** (see the claim above): preserving and addressing an
  artifact the deputy rendered is the ledger knowing where its evidence is. The line is drawn at
  rendering — the day prova draws a coverage table or a trend chart, this boundary has moved and
  should be moved deliberately, not discovered in a diff.
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

## Field reports (Substrate gate-integration run, 2026-08-11)

<!-- claim: conduct-heartbeat-not-deadline recorded=2026-08-11 -->
**A conduct can be supervised by liveness: `shell.run { idle_timeout }` bounds death, never
work — and the heartbeat is bytes OR CPU.** Bound the resource, never the work: a clock is
legitimate as a sampling rate, illegitimate as a task budget, and conducts run
externally-sized work by design. Silence on the pipes is only half the evidence — a big
crate's codegen says nothing for minutes while working flat-out (observed live: a 120s
byte-only bound killed a healthy compile) — so `idle_timeout = "90s"` kills only when a
window passes with NO bytes on either stream AND no CPU progress: a genuine hang or a
lock-starved conduct (~0 CPU) dies faster than any honest deadline, a quiet-but-busy one is
never falsely killed, and the error names both absent evidences and carries the output tail
so the stall point is in the report. CPU is read natively — procfs on Linux, libproc on
macOS, `GetProcessTimes` on Windows, degrading to bytes-only where no reader exists — never
by shelling to `ps`, whose dialects would re-import the guessing this claim exists to kill.
The wall-clock `timeout` remains the optional outer bound; the two compose. (Field report
2026-08-11: a cold two-crate nextest conduct was killed at `timeout=600s` while making steady
progress — the author's guess about build time became a false failure.)

<!-- claim: suite-scoped-shared-deputies recorded=2026-08-11 -->
**`Scope.Run` — one conduct, every suite: the fifth scope crosses the Lua-state boundary as
data.** A run-scoped fixture is declared once (a `require`d package is the recipe-sharing
shape), conducted at most once per run whatever the outcome, and readable from any suite on
any worker: the run-wide store holds one slot per fixture name, whichever consumer asks first
conducts, and everyone else waits for the settled slot — lazy single-flight IS the ordering,
so a deselected reader never triggers the conduct at all. Values are plain data
(JSON-serializable; each state gets its own copy — a non-serializable return refuses naming
the constraint), factories have no `ctx:defer` (artifacts live in the tree), and failure
poisons the run instance with the one recorded error, replayed as a named memoized verdict
(the run-instance form of lifecycle.md#fixture-failure-memoization). Design and decisions:
docs/plans/shared-deputies.md. (Field report 2026-08-11: a kernel-integration proof needing
the ut lane's cases had to re-conduct its crates or read the junit artifact by path, with no
ordering guarantee.)

<!-- claim: exclusive-conduct-resources recorded=2026-08-11 -->
**The contention cure is discoverable from every door the stuck operator tries.** A capability
an agent cannot discover does not exist: when concurrent conducts starve on one tool (three
cargo-nextests on one target lock), the cure — `locks = { prova.writes("cargo") }`, the
scheduler serializes only the holders — must be named where that operator actually looks. The
sightlines: the `--jobs` help (the dial they reach for instead) points at locks; the `learn`
catalog carries a `locks` topic teaching the grammar (`writes`/`reads`/`port`), `serial`'s
run-scoped distinction, and the cross-instance file lock; the skill's authoring reference
names the same vocabulary. The mechanics themselves are architecture.md's claims
(`#locks-cross-instance`, `#lock-wrapper-verb`); this claim owns the teaching surface.
(Field report 2026-08-11: the operator diagnosed via `ps`, worked around with a global
`--jobs 1`, and filed this believing the feature missing — it had shipped, untaught.)

<!-- claim: selection-pushdown-into-conducts -->
**Selection pushes down into deputies: `prova.selection` is the run's resolved axes as plain
data, the deputy translates, and a narrowed account says so.** The engine's whole
contribution is one read-only fact — every axis of the resolved selection (keywords,
excludes, tags, nodes, lane tags, `is_empty`), present in every state — and the *deputy* owns
the translation to its framework's grammar, in the package where that knowledge lives: the
workspace's nextest deputy maps `-k` keywords to `-E test(…)` filters, so
`prova -k seed_memory -s ut` compiles to one filtered conduct under the same cargo lock,
profile, and junit adoption; a deputy that ignores the table conducts in full, and axes a
framework cannot speak never narrow. Honesty is the engine's half: a run recorded under a
non-empty selection marks its deputed account NARROWED — `evidence` says so on the DEPUTED
line, and `attest junit:…` of an absent case names the narrowing instead of implying the
case never existed. The development ladder lives in one tool — case → module → crate → suite
— each stage honoring the same house rules without the operator thinking about them.
Composes with resumable-runs-incremental-verdicts (this is the manual scalpel, that the
automatic planner) and agent-ergonomics.md#claim-scoped-selection (the claim-addressed
spelling of the same narrowing). (Substrate field report, 2026-08-11.)

<!-- claim: timeout-reaps-the-conduct recorded=2026-08-12 -->
**A timed-out conduct is dead, not merely reported dead.** Every bound `shell.run` enforces — the wall-clock `timeout`, `idle_timeout`, and their composition — kills the child process when it fires. A bound that only abandons the wait leaks the conduct: the run reports red while the child keeps running, holding exactly the locks the report just claimed were released (the observed shape: an orphaned nextest holding the cargo target lock against the next invocation). Direct child only; process-group reaping is the successor item.

<!-- claim: conduct-process-group-reaping recorded=2026-08-12 -->
**A conduct dies as a tree: every kill path reaps its process group, never just the shell.**
Each `shell.run`/`shell.spawn` child spawns into its own process group (unix), and every
controlled kill — wall clock, idle bound, `Process:stop()` — is a group kill, so a
`sh -c "a | b"` pipeline, a script's workers, or a build tool's own children can no longer
outlive the red report that claimed their locks were free. The Ctrl-C trap the naive fix
carried (children in their own group stop hearing the terminal's SIGINT) is closed by the
lease (#conduct-lease-survives-prova-death): prova's death, graceful or not, sweeps the
registered groups — so interrupt behavior gets STRONGER, catching even children that
re-group themselves, which today's shared-group accident never did. Windows keeps
direct-child kills until the windows lane lands job objects — stated, not silent. Composes
with timeout-reaps-the-conduct (the direct-child half).

<!-- claim: conduct-lease-survives-prova-death recorded=2026-08-13 -->
**A conduct's right to run is a lease enforced by something that survives prova's worst death.** Cleanup code cannot be the mechanism — the deaths that matter (SIGKILL, OOM, a panic, CI's SIGTERM) run no destructors — so the lease is held outside the dying process: on unix a reaper sidecar (`prova reap`, the same static binary) holds the read end of a pipe whose write end lives and dies with prova, keeps the registered conduct process groups, and on pipe EOF — delivered by the kernel for every death, Ctrl-C through kill -9 — sweeps them and exits; on Windows the native spelling is a job object with KILL_ON_JOB_CLOSE (the windows lane's business — until it lands, Windows keeps today's direct-child behavior, stated, not silent). Ctrl-C gets stronger, never broken: the reaper sits in its own group so the terminal's SIGINT cannot kill the janitor before it sweeps.

<!-- claim: detached-topologies-hold-no-lease recorded=2026-08-13 -->
**`prova start` provisions are deliberately unleased.** The lease's whole premise — conducts die with the run — is exactly wrong for the one verb whose purpose is outliving the invocation: a detached topology's processes register nothing, keep their `running/` record + `prova down` lifecycle, and survive prova's exit on purpose. The carve-out is the verb's, not the author's: the same factory leases under a run and detaches under `start`.

<!-- backlog: coverage-harness-uninstrumented-under-llvm-cov-0-8 recorded=2026-08-23 -->
The coverage harness measures nothing: proofs/coverage/coverage_test.lua fails with "0 suite profraw(s) — the subject is not instrumented", and a direct `cargo build -p prova-cli` under cargo-llvm-cov 0.8.7's own `show-env` environment also produces a binary with zero __llvm_prf symbols. cargo-llvm-cov 0.8.7 instruments through RUSTC_WRAPPER gated on an explicit __CARGO_LLVM_COV_RUSTC_WRAPPER_CRATE_NAMES allowlist and reports a split CARGO_LLVM_COV_BUILD_DIR; the harness's explicit --target-dir pin predates both. This is the same class of break the pin's own comment records surviving once before (show-env stopped setting CARGO_TARGET_DIR). Until it is fixed, `prova run coverage` and `prova run release` are red for a harness reason rather than a coverage reason, and the ratcheted baseline in .prova/baselines/quality.json is unenforced.
