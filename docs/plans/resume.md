# Resume — a red run costs its failures, not the suite

**Status:** plan (2026-09-22). Phase 1a is being built; 1b is designed here. **Phase 2 is built
(2026-10-29)** — see its section for what landed and where it deviates from the sketch.

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

### Extra roots

A package's runs can read bytes outside its own repository. Substrate builds fleet, typhon and
anemnez-graph through `[patch]` PATH dependencies on sibling checkouts. The VCS-only fingerprint
cannot see those, so an edit to `../fleet` between a red run and its resume would carry passes
forward across bytes that changed. That was witnessed on 2026-09-22, when a sibling edit broke a
running gate.

`[resume] roots = ["../fleet", …]` names such directories, relative to home. The fingerprint digests
each root's own repository's tracked tree beside the package's, joined as `<pkg>+<root>+…`. A root
that cannot be fingerprinted refuses the resume, as a missing VCS does.

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

### Phase 2 as built (2026-10-29)

- **Declaration.** `inputs = { "<path or glob>", … }` is a test/flow/group option (in the closed
  `UNIT_OPTS`), package-root-relative; a group's inputs are inherited by every leaf under it. A
  directory stands for everything under it. Any other shape is refused at load, naming the fix.
  The Lua-function form of the sketch is not built: globs covered every case in hand.
- **Key.** sha256 over length-prefixed parts: `inputs-v1`, the file-qualified leaf path, the
  declaring file's package-relative path and its bytes' digest, the prova VERSION (not the binary
  fingerprint, which carries the executable's path and mtime and would never match across
  checkouts), and a ROOT-RELATIVE digest of the inputs (`modules::digest_paths_relative`: each
  matched file's relative path, length and bytes, sorted; a pattern matching nothing contributes
  its text behind an absence marker). Two checkouts of the same bytes compute the same key;
  `fs.digest`'s absolute digest is untouched.
- **Store.** One JSON file per key (`{schema, path, run_id, at}`), written to a temp name and
  renamed, under `.prova/var/reuse/` — or, with `PROVA_VAR_DIR` set, `$PROVA_VAR_DIR/reuse/` at the
  override ROOT rather than the package-keyed dir beneath it, because that dir is keyed by the
  checkout's path and the point is for a fresh checkout (Herald's) to carry from the one that
  earned the pass. Every run that can honestly file one does — a plain gate seeds the store — and
  only an EXECUTED `passed` is filed (never a failure, skip, promise, or a pass the run itself
  carried, so an origin is always the executing run). Entries older than 30 days are pruned when a
  run files. Nothing is filed under `--falsify`, `--dep`, `--update-baseline` or `-u`.
- **Carrying.** `--reuse` looks up every input-declaring leaf's key; matches join the same filter
  `--resume` uses, so dependency closure (a carried leaf an executing leaf depends on runs again)
  and flow atomicity hold unchanged. It composes with `--resume` (a path either would carry is
  attributed to `--resume`, the stronger warrant) and with narrowing (`-k` etc.). One stderr line
  says `--reuse: N of M input-declared leaves carried forward`; zero matches simply executes.
- **Refusals (exit 2).** `--reuse` with `--update-baseline`, `--falsify` or a `--dep` override, or
  with no package home to hold a store.
- **Record (schema 4).** A carried leaf is `Executed::Reused` with `reused_from` naming the origin
  run, as in 1a, plus `reused_inputs: { path → key }` marking it input-keyed; `summary.reused_inputs`
  counts them (`reused` stays the total). The console prints `N reused (same tree)` and
  `M reused (inputs unchanged)` separately. `attest` accepts it as evidence and says "reused
  because its declared inputs are unchanged", never "identical tree".
- **Open.** An input-carried leaf is not journaled, so a later `--resume` of that tree re-executes
  it (unless `--reuse` is also given) rather than relabel an input-keyed pass "same tree". A carried
  leaf that would have SKIPPED here (an unmet `requires`) reads as reused, as under `--resume`. The
  declaration is the author's whole warrant: a helper module the proof `require`s, a subject binary
  built from the tree, a remote service — anything the verdict reads must be in `inputs` or the
  leaf must not declare them. Proven black-box by `proofs/resume/reuse.prova.lua`.

## Honesty rules (every phase)

- Reused never reads as executed: in the record, the console and `attest`, it names its origin.
- A key mismatch is a refusal, never a silent full run and never a silent partial one.
- Reuse only ever carries a **pass** forward. A failure always re-executes.
