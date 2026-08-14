# Incremental prova — conduct identity, and not paying twice

Status: **drafted 2026-08-13; governing principle ratified same day** (§The line). A joint design for
three shelf items that are one mechanism seen from three sides:

- `lifecycle.md#resumable-runs-incremental-verdicts` — a failed sweep re-pays the whole world;
- `manifest.md#subject-provisions-at-first-read` — a run pays for a subject no selected proof reads;
- `agent-ergonomics.md#dedupe-identical-deputy-conducts` — two deputies conduct the same cargo twice.

Companion to [shared-deputies.md](shared-deputies.md), whose `Scope.Run` store this extends, and to
[verifiers.md](../design/verifiers.md), whose conducts are the expensive things being counted.

## The problem, measured

One `prova run all` on this tree, 2026-08-13 (829.3s wall, 636 proofs):

| Conduct | Wall |
|---|---|
| `cargo nextest run --workspace` (the ut deputy) | 319.6s |
| `cargo clippy --workspace --all-targets -- -D warnings` | 263.1s |
| `cargo clippy --workspace --lib --bins` (restriction lints) | 77.9s |
| 3 × `cargo build` (sandbox subjects) | ~54s |
| **everything else — all 636 proofs** | **~115s** |

**93% of a sweep is four cargo conducts; all 636 proofs together are two minutes.** Within one run
that is already optimal — `Scope.Run` conducts each once. The waste is *across* runs: a docs-only
edit, or a re-run after one ratchet regression, re-pays 660s to re-ask questions whose inputs did
not change. Six sweeps landed this session's work and five re-earned an identical ut verdict.

## The line: cache conducts, never proofs

Prova is black-box by thesis: a proof probes a system it deliberately does not introspect — a
container, a local daemon, a remote service, the machine's own integration. So **a proof's input
set is not enumerable in principle**, and any digest standing in for it is wrong in both
directions: too narrow (an image tag moves, a remote ships, a `brew upgrade` lands — the tree is
identical and the answer is not), too broad (one edit invalidates everything). A cached green on a
proof is exactly the lie prova exists to prevent; no care in choosing the digest repairs a category
error.

A **conduct** is a different animal. `cargo nextest`, `cargo clippy`, `docker build` are tools over
inputs someone can name. Not pure — but *nameable*, which is the property that matters.

The tell that this is the real seam: the principled boundary and the valuable boundary are the same
boundary. Conducts are 93% of the sweep. **Proof-verdict caching is never needed and never built.**

## Who declares a conduct's inputs: the deputy

Settled by precedent. `verifiers.md#selection-pushdown-into-conducts` ratified the split — the
engine contributes one read-only fact, and the *deputy* owns the knowledge of its own tool, in the
package where that knowledge lives. Input knowledge is that same kind of knowledge:

- not the **engine**, which would be guessing at cargo's input set behind a tree digest;
- not the **proof author**, who does not know it either;
- the **deputy**, written once by whoever chose the tool. There are a handful, not hundreds.

**The tool's own version is part of the identity.** Not paranoia — observed 2026-08-13: identical
tree, identical argv, different verdict from a January nightly clippy versus CI's stable. A tool is
an input that does not live in the tree.

## The model: a `Scope.Run` fixture that declares what it depends on

Knowledge in the package, mechanism in the engine:

```lua
M.nextest = prova.fixture("nextest-junit", Scope.Run, function() … end, {
  inputs  = { "crates/**", "Cargo.toml", "Cargo.lock" },   -- what changes the answer
  outputs = { "target/nextest/prova/junit.xml" },          -- what the answer leaves behind
  tools   = { "cargo --version", "cargo nextest --version" }, -- inputs that live outside the tree
})
```

- **identity** = digest(inputs' contents, tools' stdout, the factory's own source, argv);
- **replay** happens when a persisted slot matches the identity **and** every `outputs` path still
  exists — `inputs` decide whether the answer changed, `outputs` decide whether it is still there
  (a `cargo clean` or a pruned `target/` must re-conduct, identity notwithstanding);
- **absent `inputs`**, a `Scope.Run` fixture behaves exactly as it does today: conducted once per
  run, never replayed. Opt-in, and silence means the current semantics.

This generalizes past deputies with no extra concepts: a built image, a rendered archetype, any
expensive `Scope.Run` value gets replay by declaring what it depends on.

## The honesty rule: a replayed conduct is recorded, not hidden

A replayed conduct is the one thing here that can green-wash, so it is marked with the mechanism
prova already uses for partial evidence — the NARROWED deputed account:

- the account's deputed row records **REPLAYED**, with the identity's age;
- `attest` accepts replayed evidence (the tool did run, over these inputs, at this version) but
  reports it as replayed, exactly as it reports narrowed;
- the narration says so at conduct time (`adopting a replayed artifact — inputs unchanged`);
- `--reconduct` forces re-execution, the analogue of `--reprovision`.

## Resume is an account state, not a cache

A resumed run is *partial evidence*, which prova already has vocabulary for. `--resume` re-executes
only what was red, unattested, or absent, and the run records a **RESUMED** account that `attest`
treats exactly as it treats NARROWED. No cache, no staleness question, no new honesty mechanism —
the third reuse of one already proved.

## The subject is a fixture

`[runner]` provisioning is a bespoke reimplementation of `Scope.Run`: laziness, single-flight under
`-j`, taking the package's cargo lock, memoizing failure, an invalidation flag (`--reprovision`),
and a stamp file that is a crude conduct identity. Express the subject as a built-in `Scope.Run`
fixture with `inputs = [runner].sources` and every one of those becomes an instance of an existing
concept — `subject-provisions-at-first-read`'s "wants design, not just plumbing" evaporates, because
the conduct store already solved single-flight. It deletes bespoke code rather than adding
machinery, which is the usual signal of the right abstraction.

## Increments

1. **Conduct identity, in-run.** The digest primitive, and the `Scope.Run` store keyed by identity
   as well as name. Closes `dedupe-identical-deputy-conducts`; no persistence, no honesty question.
2. **Cross-run replay + REPLAYED.** Persist identity→value in `var/`, replay on match with outputs
   present, mark the account, add `--reconduct`. The measured win: a docs-only re-run drops from
   829s to ~120s.
3. **The subject as a fixture.** Re-express `[runner]` provisioning on 1–2; delete the stamp file,
   the bespoke lock handling, and the eager provision. `--reprovision` becomes `--reconduct` scoped
   to one slot.
4. **`--resume` + RESUMED.** Journal per-node outcomes; re-execute only what is not known-green.

1 is the substrate; 2 is where the money is; 3 pays down code; 4 is independent. Everything after 1
is optional in the sense that it can stop there without owing anything.

## Ratified: `fs.digest` ships as an engine primitive

Not a deputy-side shell-out. `git hash-object` and `sha256sum` are absent on a bare Windows runner
and differ in flags across the BSD/GNU line — a package that computes identities by shelling out is
a package that works on the author's box. Prova's pitch is one static binary that behaves the same
everywhere, so the belt carries the battery: `fs.digest(paths)` beside `hash.sha256`, over the same
`sha2` already in the tree (zero new dependencies).

Contract: a stable lowercase hex digest over the CONTENTS and package-relative PATHS of the files
matching `paths` (a path or glob, or a list of them), resolved in sorted order with `/` separators,
so the same tree digests identically on every OS. A missing path is part of the answer, not an
error — absence changes the digest, because absence changes the build.
