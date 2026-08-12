# Shared deputies — run-scoped conducts, and selection that rides along

Status: **drafted 2026-08-12; all four decisions ratified same day as recommended** (§Decisions:
lazy block-on-first-use, adoption-by-doctrine, narrow-by-default, `Scope.Run`). The joint design for two shelf
items that turned out to be one mechanism seen from two sides:
`verifiers.md#suite-scoped-shared-deputies` (one conduct, many suites) and
`verifiers.md#selection-pushdown-into-conducts` (one selection, every granularity). Companion to
[verifiers.md](../design/verifiers.md) (conduct-once-read-many is the pattern this generalizes)
and [lifecycle.md](../design/lifecycle.md) (fixture-failure memoization is the semantics this
inherits).

## The problem, precisely

"Conduct once, read many" stops at the suite boundary, and the boundary is architectural: a
directory with a `suite.lua` is one suite sharing one Lua state; a directory without one makes
**every file its own singleton suite**; and under `-j`, suites run on parallel workers with
separate Lua states. A fixture *instance* — a Lua value — cannot cross that line. So the ut
lane conducts the whole workspace, and a kernel-integration proof in another directory must
either re-conduct (double cargo) or read the ut lane's junit artifact by path, with no ordering
guarantee and no freshness story. Meanwhile selection is proof-granular: "run exactly this one
case" drops to raw `cargo nextest -E` outside prova, where locks, profiles, and the account do
not follow.

The misnomer in the shelf item is "suite-scoped": the suite already has a scope. What the case
needs is **run-scoped** — declared once, conducted once per run, adoptable from any suite on any
worker, ordered by construction.

## The model: `Scope.Run`, the fifth scope

One new member of an existing vocabulary, not a new concept. `prova.fixture(name, Scope.Run,
factory)` is an ordinary fixture whose scope instance is the whole run:

```lua
-- .prova/packages/deputies/init.lua — the recipe, required from any suite.
local M = {}
M.nextest = prova.fixture("nextest-junit", Scope.Run, function(ctx)
  local artifact = prova.root .. "/target/nextest/prova/junit.xml"
  fs.remove_all(artifact)
  shell.run({ "cargo", "nextest", "run", "--workspace", "--profile", "prova" },
    { cwd = prova.root, idle_timeout = "120s", locks = ... })
  return artifact
end)
return M
```

- **The store crosses workers the way every cross-cutting account already does**: an
  `Arc<Mutex<…>>` registry carried by `RunConfig`, the exact pattern of `SnapshotRegistry`,
  `DeputedRegistry`, and `MeasurementRegistry`. Slots are `Conducting | Ready(value) |
  Poisoned(error)`; a worker that finds `Conducting` blocks on a condvar until the slot settles.
- **Values are data, not Lua values.** A `Scope.Run` factory's return crosses Lua states, so it
  must be JSON-serializable (in practice: an artifact path, or a small table of paths and
  metadata). A non-serializable return is an error at provision time naming the constraint. This
  is the honest cost of run scope, stated once, at the boundary.
- **Failure memoizes exactly as it does everywhere** (lifecycle.md#fixture-failure-memoization):
  the recorded error poisons the run instance, and every later consumer on every worker replays
  it as a named memoized verdict. One dead cargo, one payment — now across suites, not just
  within a file.
- **Scope rank**: Run sits above Suite. A suite-scoped fixture may `use` a run-scoped one; the
  reverse is the existing scope-mismatch error.
- **Teardown**: run-scoped defers run at run end, LIFO, owned by the engine's conclusion — after
  every suite's teardowns, before the record is written. (Most conducts need none; the artifact
  is the point.)
- **Single-flight is in-process on purpose.** `-j` workers are threads in one run. Two prova
  *instances* are two runs and must not share verdicts (freshness doctrine, verify-freshness);
  what serializes them at the tool level is the existing lock machinery (`writes("cargo")`),
  which already spans instances. The two mechanisms compose; neither replaces the other.

## Ordering — the question dissolves

The shelf item asked for "ordering semantics" because readers consumed a raw artifact path.
Under `Scope.Run` no reader touches a path it didn't get from `t:use` — and lazy, blocking,
single-flight provision *is* the ordering: whichever worker asks first conducts; everyone else
waits for the settled slot; a reader that is deselected never triggers the conduct at all (the
granular-efficiency posture, same as every fixture). Alternatives considered and rejected:
explicit `depends_on` edges (lifecycle.md already ruled: sequencing enforced by shared facts
beats sequencing enforced by graph edges) and an eager pre-pass conducting all run-scoped
fixtures up front (pays for deputies the selection never reads).

## Sharing doctrine, restated

- **Recipe** sharing is `require`, exactly as today — the deputy lives in a local package; the
  `shared` package's "registers fixtures, returns handles" pattern is the shape.
- **Instance** sharing within a run is `Scope.Run`.
- Across runs and across machines there is no sharing — that is `resumable-runs-incremental-
  verdicts`' verdict-caching territory, deliberately out of scope here.

Two footguns get fixed regardless of the rest (they bite today):

1. **Duplicate fixture names become an error**, exactly as `prova.topology` already validates.
   Today two files declaring the same-named fixture silently fork into two instances — the
   copy-paste "sharing" that quietly doubles a conduct. With `Scope.Run` a silent fork would be
   worse: a name is now a run-wide contract, so an ambiguous one is a defect (the duplicate-
   claim-id precedent).
2. **`Scope.Suite` in a singleton-suite file warns**: a file that is its own suite (no
   `suite.lua`) gets file-scope behavior from a suite-scoped declaration — legal, but almost
   never what the author meant. The diagnostic names the fix (add `suite.lua`, or say
   `Scope.File`).

## Selection pushdown — expose facts, let the deputy translate

The engine's whole contribution is one read-only fact: **`prova.selection`** — the run's
resolved selection (`keywords`, `keyword_excludes`, `tags`, `tag_excludes`, `nodes`, and the
lane's baked tags), the same `Selection` every lane already speaks. No callback protocol, no
filter IR: the *deputy* owns the translation to its framework's grammar, in the package where
that knowledge belongs — the nextest deputy maps keywords to `-E 'test(/…/)'`, a pytest deputy
to `-k`, and a deputy that ignores the table simply conducts in full (today's behavior,
unchanged by default).

Honesty is the engine's half. A conduct performed under a non-empty selection produces a
**partial account**, and partial must never wear full's face:

- Deputed rows from a narrowed conduct are recorded `narrowed: true`; the tally's deputed line
  says `(narrowed)`.
- `prova attest junit:<suite>#<case>` against a narrowed account fails for any case the
  narrowing excluded — same contract as a deselected proof: not run is not attested. The CI
  gate is unaffected because CI runs unnarrowed.
- The store keys by name alone: within one run the selection is a constant, so a narrowed
  conduct and a full one can never collide in the same run.

The development ladder this buys: `prova -k seed_memory` (with the ut switch thrown) compiles to
one filtered nextest conduct under the same cargo lock, same profile, same adoption — one case,
one module, one crate, one suite, all one vocabulary, none of it re-taught per bypass.

## Increments

1. **`Scope.Run`** — scope + rank, the conduct store in `RunConfig` (registry pattern), blocking
   single-flight, JSON-value constraint, run-instance poisoning, run-end teardown. Proofs: two
   suites in one run conduct once (marker-counted); poison replays across suites; a
   non-serializable return errors naming the constraint; Test/Suite semantics untouched.
2. **Footguns** — duplicate-fixture-name error; singleton-suite `Scope.Suite` diagnostic.
   Proofs: both messages, and the duplicate error names both declaration sites.
3. **Dogfood** — the nextest deputy moves to a `.prova/packages/deputies` package at `Scope.Run`;
   a second suite binds a claim through it to prove the cross-suite read; the copy-pasted
   `project()`/`package()` proof scaffolds move to a shared package in the same pass (recipe
   sharing, no engine work — the cleanup that motivated the question).
4. **`prova.selection`** — the read-only table, plus the nextest deputy's translation. Proofs:
   a narrowed run conducts a narrowed deputy (observed via the deputy's own command journal);
   an ignoring deputy still conducts full.
5. **Partial-account honesty** — `narrowed` on deputed rows, the tally marker, attest's refusal.
   Proofs: attest a case the narrowing excluded → red naming the narrowing; the unnarrowed CI
   shape unchanged.

## Decisions

- **D1 — ordering**: lazy block-on-first-use (recommended, above) vs eager pre-pass vs edges.
- **D2 — adoption uniqueness**: keep "one proof adopts, siblings load" as doctrine (recommended;
  a second `junit.verify` of the same artifact double-ledgers deputed rows), or have the record
  dedupe adopted cases by (verifier, artifact, case) so double-adoption is harmless.
- **D3 — pushdown posture**: the shipped nextest deputy narrows whenever `prova.selection` is
  non-empty (recommended: the scalpel works out of the box; honesty machinery keeps it safe) vs
  narrow only behind an explicit opt (`deputies.nextest { pushdown = true }`).
- **D4 — spelling**: `Scope.Run` (recommended: fifth member of an existing vocabulary; every
  fixture law carries over verbatim) vs a distinct `prova.deputy(name, …)` declaration (a new
  noun, but it could carry deputy-specific affordances later — e.g. a declared artifact glob).
