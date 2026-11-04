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
from  = ["cargo", "swiftpm"]        # the adapters: a bundled name, or a [dependencies] package
reads = { "cos-daemon" = ["vault/recipes/**", "vault/checks/**"] }   # run-time reads by tests only
```

### 2. The adapter answers one question

An adapter is a package, resolved like any other `require`: a bundled `prova.<name>` first, then a
`[dependencies]` package of that name, so a team writes its own for a build tool Prova does not
ship. A repository may use several (Substrate needs `cargo` for its crates and `swiftpm` for
COSKit); a directory claimed by two adapters is refused at load, naming both. The contract is
small, and Prova's own suite proves every bundled adapter against it:

| Function | Returns |
|---|---|
| `modules(root)` | every module: `{ name, dir }` |
| `closure(name)` | every path `name`'s tests are built from or read (below) |
| `conduct(names, ctx)` | one tool run over those modules, returning `{ [name] = account path }` |
| `census(name)` *(optional)* | the reads a traced run made outside `closure(name)` |

For module `m`, `closure(m)` is every path whose bytes `m`'s tests are built from or read. For
Cargo:

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

### 3. Cache the conduct, never the proof

This is the line incremental-prova.md drew, kept exactly. What costs is the conduct (nextest's
build and run), never the proofs that read its account, so **the conduct is what is cached, per
module, and a module proof always executes**: it parses an account, which costs milliseconds.

```lua
local cargo = require("prova.cargo")
prova.test("scm's unit cases pass", { switch = "ut" }, function(t)
	cargo.verify(t)                                -- this module's account, adopted
end)
```

`cargo.verify(t)` adopts `prova.module`'s account; `cargo.account("cos-systems-epistemic")` hands a
proof anywhere another module's account to bind claims to its named cases. No author declares
`inputs`: the key is the adapter's, computed from the graph.

**The conduct cache.** Under the reuse store (`$PROVA_VAR_DIR/conducts/` when set, so a fresh
checkout, Herald's, carries what a lane earned), the adapter files a module's account after a
conduct in which **every one of its cases passed**, keyed by `sha256(adapter, module, its closure's
root-relative digest, prova version)`. Nothing else is filed: a red, a narrowed conduct, or a run
under `--falsify`, `-u` or `--update-baseline`.

**One conduct per run, over every stale module.** The first `verify`/`account` call in a run (a
`Scope.Run` fixture: lazy, single-flight across workers) computes every module's key, finds the
stale ones (no filed account under the current key), and conducts them in one process:
`cargo nextest run --workspace -E 'package(a) | package(b) | …'`. One workspace build, so no `-p`
feature drift and no recompile per module; one nextest, so every case is scheduled across all
cores. Fresh modules' accounts come from the cache, each case marked `reused_from` with the run
that executed it (as `junit.merge` already marks a resumed case). Under a non-empty
`prova.selection` (`-k`, `--node`), the conduct is narrowed to what was asked for, and marked so,
as shared-deputies.md requires.

**Dependents fall out of the key.** `cos-daemon`'s closure contains `cos-systems-graph`'s lib
dep-info, so an edit there changes `cos-daemon`'s key and it is conducted; a sibling the edit cannot
reach keeps its key and its cached account. No second "dependents" pass exists to get wrong.

Phase 2's `inputs` stays for what it is for: a leaf whose own work is expensive and whose inputs
its author can name. A module proof does not need it.

### 4. Seeing why

`prova why <proof>` prints the proof's module, the account's key, and either the run its account
came from or the first closure path that differs from the filed one
(`crates/cos-systems-graph/src/edge.rs`). The run's summary counts conducted and cached modules.

## Honesty rules (beside Phase 2's)

- **Cached never reads as conducted.** Every case from a cached account carries `reused_from`, the
  record and the console count conducted and cached modules apart, and `attest` names the origin run.
- **An adapter that cannot read the graph runs every module, and is noticed** (lesson 3). The
  causes differ, and none is covered: the change broke the build's own files (a run that executes
  everything then goes red on the build's own error, which is the change's verdict); the host lacks
  the tool or its network (infrastructure, as a full disk is); or the adapter has a bug (Prova's).
  The record carries `selection: off` with the adapter's error, the summary prints it, and the run's
  JSON names it, so a gate reports it as an infrastructure event (Herald notifies the gates lane),
  never as a silent slow run and never as a narrower one. The Cargo adapter reads with `--locked
  --offline`, so reading the graph never needs the network.
- **The full run is the audit.** A run with the conduct cache off (`--fresh`, nightly, and before a
  release) conducts every module and is the ground truth. A cached account the audit turns red is a hole in a closure: a gate-reliability
  defect, fixed by class, never called a flake.
- **A run-time read nobody declared is found, not trusted away.** The adapter ships a census (as
  Substrate's `library_embeds` does for embeds): a test run under file-access tracing whose reads
  fall outside its module's closure is named.

## What retires in Substrate

`PROVA_AFFECTED`, `PROVA_AFFECTED_EXACT`, `outside_scope` and `lib.scope_skips`, the crate half of
`vault/lib/affected.lua` and its embeds table. Its reads rows move to `[modules] reads`. The proof
half (a proof that names a vault script it drives) is a black-box proof and always runs, as every
proof does; only conducts are cached.

## Phases

1. **Prova engine:** `[modules]`, `prova.module`, the conduct cache (a store primitive with the
   honesty rules above: filed only from an all-green, unnarrowed, executing conduct; pruned at 30
   days; `--fresh` bypasses it), and `[dependencies] module` renamed `entry`.
2. **The `cargo` adapter:** `closure()` from metadata + dep-info + reads, and `verify()`/`account()`
   over the one batched conduct of stale modules. Proven on a fixture workspace (`a → b → c`, `c` embedding `data/x.txt`,
   `b`'s tests reading `data/y.txt`): an edit to `c` conducts all three, an edit to `a` conducts only `a`,
   `data/x.txt` conducts all three, `data/y.txt` conducts only `b`, a `Cargo.lock` bump conducts all three, a
   file under `c/target` conducts nothing, an unreadable graph conducts all three loudly. Each case is first
   seen red against an adapter that leaves the path out.
3. **Substrate pilot:** two leaf crates' proofs move into their own `proofs/`; the workspace deputy
   runs the rest. Each pilot gate is compared with a full run of the same commit until ten agree,
   and gate time on a leaf-crate change is measured.
4. **Substrate, crate by crate**, `cos-daemon` last; the workspace deputy and the affected scope
   retire when nothing reads them.
5. **`prova why`; the read census; more adapters** (swiftpm for COSKit, maven, uv, npm) from need.

## Decisions (User, 2026-11-04)

- **D1, the word: "module"**, for the build tool's unit; "package" stays a Prova package.
  `[dependencies] module = "…"` (an entry file) is renamed `entry =`, as `[package] entry` spells it.
- **D2, adapters: `prova.cargo` bundled, any other a package**, and a repository may use several
  ("we need a way to support different adapters"): the contract in §2.
- **D3, an unreadable graph: run everything, and it is noticed** (§Honesty). The User asked whether
  it is a bug being covered; it is one of three causes, each surfaced, none hidden.
- **D4, reads: `[modules] reads` in the root manifest**, checked by the adapter's census.
