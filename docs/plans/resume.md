# Resume — a red run costs its failures, not the suite

**Status:** plan (2026-09-22). Phase 1a is being built; 1b and 2 are designed here and not started.

## The cost this removes

Substrate's `prova run all` takes 17–22 minutes (measured on the Studio, 2026-09-21/22), and
three events currently cost that whole time again:

1. **One red case.** It is usually a load flake. The only honest re-attestation today is the
   whole lane, because `--last-failed` re-runs the failures and attests nothing about the rest.
2. **A killed run.** A daemon restart, a sleep or an OOM loses every verdict produced so far. The
   record is written once, at the end.
3. **A re-gate under a commit window.** Substrate's fleet now re-gates an overlapping landing
   while holding the fleet-wide landing lease, so every other machine's landing waits the length
   of that gate.

## The principle

A verdict is reusable exactly when **what produced it is unchanged**, and the record says it was
reused instead of pretending it ran. Every phase is the same rule with a finer key.

- **Phase 1** uses the coarsest sound key: the whole tree, the prova binary and the selection.
- **Phase 2** narrows the key to what a leaf declares it reads.

## Phase 1a — same-tree resume at the leaf

`prova run <lane> --resume` executes only the leaves that did not pass in the previous run of the
same lane over the same tree. Every other leaf is **reused**.

- **Tree fingerprint.** The record gains `tree`: a digest of every file the VCS tracks, covering
  path and bytes. jj lists them with `jj file list`, git with `git ls-files -z`; ignored and
  untracked files are out. It is content, not a commit id: a `jj describe` does not change it,
  and an edited file always does. With no VCS there is no fingerprint, and `--resume` refuses.
- **Journal.** Each run appends one JSON line per settled leaf to `.prova/var/journal.jsonl`,
  after a header line carrying `run_id`, `tree`, `binary`, the spelled selection and the start
  time. A killed run leaves a valid prefix, and resume reads that prefix. A complete run's
  journal and its `last-run.json` agree.
- **Matching.** Resume takes the newest journal whose `tree`, `binary` and selection all equal
  this run's. Any mismatch **refuses** with exit 2 and names what changed. A resume that silently
  ran everything would hold its caller for the full suite while it believed it was resuming.
  The refusal teaches the two honest alternatives: run the lane, or use `--last-failed` for an
  inner loop that attests nothing.
- **What executes.** Every leaf the prior run did not record as passed executes: failed,
  skipped, promised and never reached. Their dependencies are pulled in as they are today.
- **What the record says.** A reused leaf is `Executed::Reused { from: <run_id> }`. `attest`
  counts it as evidence, because the same tree produced it. The summary gains `reused`, and the
  console prints `N reused from run <id> (same tree)`. A resumed record is COMPLETE (every leaf
  has an outcome), so a resume of a resume works, and each reused row keeps the run that first
  executed it.
- **What a resume refuses to do.** It will not update baselines (`--update-baseline`), since a
  reused leaf took no measurement. It does not re-evaluate reminders, carrying the prior rows
  forward as a narrowed run does.

## Phase 1b — resume reaches the deputy

The expensive leaf is often a **deputy that conducts a whole suite**: Substrate's
`lib.workspace_junit` runs the workspace's nextest once, about 10 minutes, and one leaf adopts
every case. With 1a alone, a single red nextest case re-runs the whole deputy.

- `prova.resume` in Lua is `nil` unless the run is resuming. When it is, it holds the prior
  run's id and `deputed(verifier)`, which returns the prior run's adopted rows for that
  verifier.
- `junit.verify(t, { results = …, reuse = rows })` adopts the new conduct's cases plus the
  reused rows that the new conduct did not re-run. Reused rows are marked reused, with the
  prior artifact as provenance. A merged junit file is never written, because that artifact
  would claim cases ran that did not.
- Substrate's deputy narrows its nextest to the prior failures (a filterset of
  `test(=…)`), so a flake-red gate resumes in about a minute instead of 18.

## Phase 2 — input-keyed reuse across trees

A leaf or deputy declares `inputs`: globs, or a Lua function that returns paths. Its verdict key
is then the digest of those inputs' bytes, the binary and its own declaring file. With `--reuse`,
any prior green whose key matches is reused, even though the tree around it changed.

- Undeclared inputs are never reused, so the default stays the sound one.
- For Substrate, this is per-crate nextest deputies whose inputs are the crate's dependency
  closure, from `cargo metadata` plus `Cargo.lock` and the toolchain. A rebase onto a sibling's
  disjoint chain then re-runs only the crates that chain can reach. This is the phase that
  shortens the commit-window re-gate.

## Honesty rules (every phase)

- Reused never reads as executed: in the record, the console and `attest`, it names its origin.
- A key mismatch is a refusal, never a silent full run and never a silent partial one.
- Reuse only ever carries a **pass** forward. A failure always re-executes.
