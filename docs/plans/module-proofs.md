# Module proofs: a change runs exactly the proofs it can reach

Status: **drafted 2026-11-04 (E), at the User's direction**: "smart resolution based on dependency
changes sounds like the difference between good and great", and "the right shape instead of Rube
Goldberg machines". It builds [resume.md Phase 2](resume.md#phase-2-as-built-2026-10-29), whose
sketch named it ("per-crate nextest deputies whose inputs are the crate's dependency closure, from
`cargo metadata`") and left it unbuilt. Four decisions are open (§Decisions).

## The cost this removes, measured

Substrate's gate, 2026-11-04 (commit c5bfa9e2, 23.3 min wall):

| Part | Wall | Share |
|---|---|---|
| `cargo nextest run --workspace` (one `Scope.Run` deputy, 8,942 cases, 5,654 test-seconds) | 14.5 min | 62% |
| COSKit + iOS builds and suites | 3.0 min | 13% |
| clippy, whole workspace | 1.5 min | 6% |
| everything else (1,787 proofs) | 4.3 min | 19% |

The workspace deputy is one conduct, and its input is the whole workspace, so Phase 2 can never
carry it: an edit to a leaf crate re-pays `cos-daemon`'s ~3,800 test-seconds, which no edit there
could change.

## What already exists, and is composed, not replaced

Every mechanism this needs is built. The plan joins them.

- **The line** ([incremental-prova.md](incremental-prova.md#the-line-cache-conducts-never-proofs)):
  a proof's inputs are not enumerable by the engine; an undeclared proof is never reused. Unchanged.
- **Input-keyed reuse** (Phase 2): a leaf that declares `inputs` is carried when their digest is
  unchanged, the store crosses checkouts (`PROVA_VAR_DIR`), only an executed pass is filed, and a
  carried leaf reads as reused, never executed.
- **`Scope.Run`** ([shared-deputies.md](shared-deputies.md)): one conduct per run, lazy, blocking,
  single-flight, failure-memoized, adoptable from any suite. A deselected reader never triggers it.
- **Selection pushdown and partial-account honesty** (same doc): the engine exposes one read-only
  fact, the deputy translates it to its tool's grammar, and a narrowed account is marked narrowed,
  so `attest` refuses a case it excluded.
- **Conduct once, read many** ([verifiers.md](../design/verifiers.md#conducting-an-expensive-deputy--conduct-once-read-many)):
  one proof adopts a deputy's whole report, siblings bind claims to named cases by `junit.load`.
- **Packages** ([package-system.md](../design/package-system.md)): tool knowledge lives in a Lua
  package (Tier 1), never in the engine; `pkg.dir` locates a package's own files.

## Prior art: Substrate's affected scope, and what it taught

Substrate built this by hand, in two halves that do not meet:

- `PROVA_AFFECTED` / `PROVA_AFFECTED_EXACT` narrow the workspace deputy to named crates (plus
  dependents, unless the caller already closed the set), and readers of a case outside the scope
  skip with a reason (`lib.affected`, `outside_scope`).
- `vault/lib/affected.lua` (870 lines) maps changed files to crates and proofs: proofs that name a
  changed path, the transitive dependents of edited crates, and a hand-kept table of ~40 rows for
  files outside a crate that a crate **embeds** (`include_str!`, `proto/**` compiled by `build.rs`)
  or that only its **tests read** at run time (`vault/recipes/**` read by cos-daemon's tests).

Each lesson is a rule here:

1. **The closure is the dependents, not the edited crate** (gap 01a16363: a pre-gate over the edited
   crate alone attested green, and the land gate went red on a crate that depended on it).
2. **Embeds and reads differ.** An embedded file is part of a library, so it reaches every dependent.
   A file only a crate's tests read reaches that crate's tests and nothing else.
3. **When the graph cannot be read, run everything, loudly.** A narrower set must never sign a cover.
4. **A deleted crate is not an error and runs nothing**; a crate no lane runs is reported, not dropped.
5. **What a proof drives through the wire is invisible to any graph.** That is what black-box proofs
   at the root are for, and why they declare nothing and always run.

Half of the hand table is derivable: rustc already writes every file a target embeds
(`include_str!`, `include_bytes!`, `build.rs` `rerun-if-changed`) into the target's dep-info
(`target/**/deps/*.d`). Only run-time reads by tests stay declared, and a census checks that.

## Words

A **module** is a build tool's unit: a Cargo package, a Maven module, a SwiftPM target, a Python
or npm workspace member. A Prova **package** stays what it is: a Lua module that returns a
namespace. A module's proofs live in `<module>/proofs/`. The root `proofs/` stays the readable
map of what the whole system promises: black-box, quality, clients, censuses.

## Shape

### 1. A proof belongs to its module

`proofs = ["proofs"]` already discovers every `proofs/` directory below the root, so
`crates/cos-systems-scm/proofs/` is found today. New: when the manifest names a module adapter,
each proof file belongs to the **innermost module directory containing it** (a root `proofs/`
belongs to none), and `prova.module` is `{ name, dir }` or `nil`. A module is never a nested home:
it carries no `prova.toml`, and module proofs run in the root's one run.

```toml
[modules]
from  = "cargo"                     # the adapter (cargo first; swiftpm, maven, uv, npm from need)
reads = { "cos-daemon" = ["vault/recipes/**", "vault/checks/**"] }   # run-time reads by tests only
```

### 2. The adapter answers one question

An adapter is a package. For module `m`, `closure(m)` is every path whose bytes `m`'s tests are
built from or read. For Cargo:

- **embeds**: the dep-info of `m`'s test targets, plus the dep-info of the lib and build-script
  targets of `m` and every crate it depends on transitively (normal, dev and build edges). Because a
  dependency's library dep-info is in each dependent's closure, an embedded `proto/**` reaches every
  dependent with no table row, and lesson 2 holds by construction.
- **reads**: `[modules] reads` for `m`, never propagated to dependents.
- **the build's own inputs**: `Cargo.lock` (registry and git dependencies are pinned there),
  every `Cargo.toml` in the closure, `rust-toolchain(.toml)`, `.cargo/config.toml`, nextest config.

Dep-info comes from the last build, which is sound: changing what a target embeds means editing a
file already in that target's closure (its source or its `build.rs`), so the key changes, the
module runs, and the rebuild refreshes the dep-info. A module with no dep-info (a fresh target
directory) has no key and runs. Every path is read through the inputs-v2 ignore rules.

### 3. Declaring: one line

`inputs` accepts the function form Phase 2 sketched and did not build, and the adapter provides it:

```lua
local cargo = require("prova.cargo")
prova.group("scm", { inputs = cargo.closure() }, function(g)          -- this module's closure
	g:test("its unit cases pass", { switch = "ut" }, function(t) cargo.verify(t) end)
end)
```

A group's `inputs` is inherited by its leaves, so a module's `proofs/` declares once. A proof
anywhere that binds claims to another module's cases declares that module's closure
(`cargo.closure("cos-systems-epistemic")`), so it is carried exactly when the cases it reads are.

### 4. The deputy: one build, one conduct, the executing modules

The engine's one new fact, beside `prova.selection`: **`prova.plan`**, the leaves that will execute
in this run after reuse and selection, each with its module. `cargo.verify(t)` reads its module's
account from one `Scope.Run` conduct, which runs
`cargo nextest run --workspace -E 'package(a) | package(b) | …'` over the modules `prova.plan`
names. One workspace build, so no `-p` feature drift and no recompile per module. One process, so
nextest schedules every case across all cores. An account from that conduct is narrowed, and
`attest` already refuses a case it excluded. Phase 1b's resume composes unchanged.

### 5. Seeing why

`prova why <proof>` prints the proof's module, its key, and either the run it would be carried
from or the first input that differs from the last pass (`crates/cos-systems-graph/src/edge.rs`).
`--reuse`'s summary line counts carried and executing modules.

## Honesty rules (beside Phase 2's)

- A module proof that declares nothing is never carried, even inside a module's `proofs/`.
- **An adapter that cannot read the graph runs every module, and says so** (lesson 3). It is never a
  silent full run and never a narrower one.
- **The full run is the audit.** A run without `--reuse` (nightly, and before a release) is the
  ground truth. A carried pass the audit turns red is a hole in a closure: a gate-reliability
  defect, fixed by class, never called a flake.
- **A run-time read nobody declared is found, not trusted away.** The adapter ships a census (as
  Substrate's `library_embeds` does for embeds): a test run under file-access tracing whose reads
  fall outside its module's closure is named.

## What retires in Substrate

`PROVA_AFFECTED`, `PROVA_AFFECTED_EXACT`, `outside_scope` and `lib.scope_skips`, the crate half of
`vault/lib/affected.lua` and its embeds table. Its reads rows move to `[modules] reads`. The proof
half (a proof that names a vault script it drives) becomes that proof's own `inputs`.

## Phases

1. **Prova engine:** `[modules]`, `prova.module`, `inputs` as a function, `prova.plan`.
2. **The `cargo` adapter:** `closure()` from metadata + dep-info + reads, and `verify()` over the
   one batched conduct. Proven on a fixture workspace (`a → b → c`, `c` embedding `data/x.txt`,
   `b`'s tests reading `data/y.txt`): an edit to `c` runs all three, an edit to `a` runs only `a`,
   `data/x.txt` runs all three, `data/y.txt` runs only `b`, a `Cargo.lock` bump runs all three, a
   file under `c/target` runs nothing, an unreadable graph runs all three loudly. Each case is first
   seen red against an adapter that leaves the path out.
3. **Substrate pilot:** two leaf crates' proofs move into their own `proofs/`; the workspace deputy
   runs the rest. Each pilot gate is compared with a full run of the same commit until ten agree,
   and gate time on a leaf-crate change is measured.
4. **Substrate, crate by crate**, `cos-daemon` last; the workspace deputy and the affected scope
   retire when nothing reads them.
5. **`prova why`; the read census; more adapters** (swiftpm for COSKit, maven, uv, npm) from need.

## Decisions

- **D1, the word.** "module" for the build tool's unit (recommended: Maven's and the User's word,
  and it leaves "package" to Prova packages). The cost: `[dependencies] module = "…"` already means
  an entry file; it would be renamed `entry =`, which `[package] entry` already spells.
- **D2, where the cargo adapter lives.** Bundled and loadable as `prova.cargo` (recommended: the
  most common adapter, versioned with the engine whose `prova.plan` it reads), with others as
  `prova-rs` packages; vs every adapter a separate package.
- **D3, an unreadable graph.** Run everything, loudly (recommended: lesson 3, and a gate keeps
  moving) vs refuse `--reuse` with exit 2 (Phase 2's posture for a key mismatch).
- **D4, where reads are declared.** `[modules] reads` in the root manifest (recommended: one file
  answers it, and the census checks it) vs a file beside each module's proofs.
