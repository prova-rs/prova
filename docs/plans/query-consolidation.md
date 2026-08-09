# Query consolidation — one lane vocabulary, one selector grammar, three surfaces that can't drift

Status: **drafted 2026-08-08.** The sequel to [terminology.md](terminology.md): terminology nailed
the *nouns*; this nails the *commands, selectors, and cross-surface parity* on top of them. Goal:
retire the git-like "every verb its own spelling" surface for a jj-like one where the shape is
guessable — you can predict a command you've never run — and where CLI, MCP, and the query engine
are provably the same surface.

## The model — three lanes, one query, two command shapes

Terminology gave us dualities; the command surface should project them as **lanes**. A lane is a
*medium* that holds items, and every item is in one of two states. There are exactly three:

| lane | medium | cold / open state | active / done state | reconciled/gated by |
|---|---|---|---|---|
| **specs** | prose (`[specs] docs`) | `backlog` | `claim` | `owed` |
| **tests** | `*.prova.lua` | `promise` | `proof` | `attest` |
| **reminders** | `prova.remind` | `watching` | `due` | `heed` |

The lane is named for its **medium**; the two states are the **duality**. This is why `specs`
(not `claims`) is the right lane name — the section already renamed `[claims]`→`[specs]` in
terminology increment 1 for exactly this reason: the medium holds *both* states, so naming it after
one state under-describes it. **The same rule settles the executable lane as `tests` (ratified
2026-08-08):** tests are the medium for promises and proofs, exactly as specs are the medium for
backlog and claim. `proofs` would name a *state*, breaking the medium-naming rule and colliding with
the promise⇄proof duality — so `proof` stays free as the done-state within the lane.

Everything the user types is one of **three roles**, and which role a word is should be guessable
from what it does:

- **Report** — `prova <lane> [--state]` **always lists**, **always carries per-item state**, exits 0,
  before or after a run. `prova reminders` is the existing exemplar (DUE / WATCHING / UNEVALUATED,
  worst-first, pre- or post-run); `specs` and `tests` join it. Bare lane = the whole lane; a `--state`
  flag narrows. A report never *does* anything but show you the lane's account.
- **Drive** — `prova <lane> <driver>` is a **red→green worklist**: it manufactures red from an
  incompleteness the default run tolerates, and exits nonzero until the work flips it green. The
  driver never writes the content — it surfaces the red and holds the line; the agent does the work.
  See *Drivers* below.
- **Run** — `prova run [<profile>]` (and bare `prova`) is the **execution engine**: it runs quality
  gates and populates the account the reports read and the drivers gate on. See *`prova run` — the
  quality account* below.

> Two rules make the whole surface guessable: **(1)** a `--flag` narrows a report, a **bare word** is
> a driver, so `prova tests --promises` reads and `prova tests burndown` drives; **(2)** one selector
> grammar (`-k` / `--tags` / `--node` / `!excl`) narrows all three roles, identically, on every lane.

There is **no `query` verb and no `list` verb** (ratified 2026-08-08). `query` would be ceremony on
every call; `list` begs "list *what?*" — the lane *is* the answer. The lane-polymorphic query is the
*engine* beneath the lane verbs, never a word the user types.

## The query engine — extract it, make it lane-polymorphic

Today there is exactly one selection engine (`Selection`, `engine.rs:88-130`) and it is well
factored: `run`/`list`/`promises`/`burndown`/`falsify` are all thin shims that prepend flags and
re-enter `run()`; `--list` reuses the identical filter trio minus execution. **But it only knows how
to enumerate the tests lane.** The specs lane (`owed`/`attest`/`evidence`/`backlog`) reconciles prose
anchors with no `Selection` at all, and the reminders rail carries its *own* 3-line selector grammar
(`ledger::ReminderEntry::matches_selector`, `ledger.rs:74`) that only "mirrors selection in spirit."
Three domains, one-and-a-fraction selector grammars. That fraction is the drift.

**The consolidation:** make selection *lane-polymorphic*. One `Query { lane, selectors, state }`
that the engine can resolve against any of the three lanes' item sources, with `-k` / `--tags` /
`--node` / `!excl` meaning the same thing everywhere. The reminders selector folds into it; the
obligation family gains selectors it never had (`prova owed -k teardown`, `prova attest --tags api`).

The lane verbs and their `--state` filters *are* the surface — **not sugar over a `query` verb, and
no convenience aliases** (ratified 2026-08-08). The reports are exactly:

| report | lists | state filters |
|---|---|---|
| `prova specs` | specs lane (backlog + claims), each item state-tagged | `--backlog` `--claims` |
| `prova tests` | tests lane (promises + proofs), each state-tagged | `--promises` `--proofs` |
| `prova reminders` | reminders lane, each state-tagged | `--watching` `--due` |

The old state-verb spellings (`backlog`, `promises`, `list`) are **removed**, not kept as aliases —
the same clean pre-1.0 cut api-freeze.md made elsewhere. A state is an *adjective* on its lane
(`prova specs --backlog`), never its own verb; keeping `prova backlog` as sugar would resurrect the
state-vs-lane conflation the model exists to kill, and would leave the asymmetry that `watching`/`due`
never got verbs. One rule, no exceptions: read a lane, filter by state.

**The cross-lane account** — `evidence` (reporter) and `owed`/`attest` (drivers) — is *not* a lane; it
reports/gates the `covers` edge between specs and tests. Parked for its own pass after the lanes land;
the lens is chosen (evidence reports, owed/attest drive — see *Drivers*).

## Drivers — red→green worklists

> **A driver manufactures red from tolerated incompleteness and holds it until the work flips it
> green — using xfail-style inversion where the condition can be honored, so neither the gap nor its
> closure can drift by silently.**

The default posture is green-biased on purpose: bare `prova` is opportunistic, an open promise reports
PROMISED (CI-green), an orphan proof complains to nobody, a DUE reminder does not fail a run. A driver
is the **opt-in strictness lens** for one lane — it refuses that tolerance and turns the silent gap
into a loud, actionable failure. So every lane has (at least) two red-states, and the driver is the
switch:

| driver | tolerated (default, CI-green) | driven red | red policy |
|---|---|---|---|
| `prova tests burndown` | promise reports PROMISED | open promise fails loud | **xfail** — body passes → "graduate to `proves`", fails until the flag's gone |
| `prova tests falsify` | vacuous body passes silently | survives-its-mutation fails | **xfail/inversion** — green-under-mutation is the vacuity failure |
| `prova specs backfill` | orphan proof complains to nobody | proof with no backing claim is red | **gate** — red until a *real* spec exists (never scaffolds empty stubs — that is vacuous green) |
| `prova reminders burndown` | DUE noted, run stays green | DUE fails | **gate** — due until the world changes; nothing to graduate |

A driver carries a **red policy**: `gate` (red until fixed) or `xfail` (expected-red, and a
surprise-green is itself a failure forcing graduation). Same shape to the agent — a worklist that
exits nonzero until green — but the xfail policy additionally guarantees no *silent completion*
(`api-freeze.md §5`'s promise semantics, promoted from a per-test flag to the defining property of the
whole category). `backfill` scaffolding empty specs would betray the whole idea: an empty spec is a
claim you've lied about covering — vacuous green, the exact thing `falsify` catches one lane over.

Naming reads the *motion*, not a forced uniformity: `burndown` *closes open items* (promises, due
reminders); `backfill` *fills a gap* (missing coverage). Different words because different motions;
same red→green shape.

Grammar consequence: **a `--flag` narrows a report; a bare word after a lane is a driver.**
`prova tests --promises` reads the open promises; `prova tests burndown` drives them. `prova <lane>
--help` lists both the states you can filter by and the drivers you can run, so every lane documents
its own verbs.

## `prova run` — the quality account, not a task runner

`just`/`xtask` run commands that produce **artifacts** (a binary, a package). `prova run *` runs
commands that produce **verdicts**, and reconciles every one into a single account. That is the
product line: **build tooling makes artifacts; prova makes and holds verdicts.**

- `prova run <profile>` runs a named **gate composition** — and a profile composes *heterogeneous*
  gate kinds, not just a selection of `.prova.lua` proofs: run these proofs, **depute** this junit
  (`prova run ut` → `cargo test`/`pytest`, verdict adopted via `junit.verify`), shell-and-check an
  exit code (`prova run clippy`), gate on a ratchet baseline (`prova run coverage`), heed these
  reminders (`prova run quality`). `[profiles.ut] deputes cargo test` must be as first-class as
  `[profiles.smoke] selects proofs=["smoke"]`. **This is the design commitment the run vision rests
  on** — it makes increment 1b (the cargo→junit bridge) and `verifiers.md` load-bearing, not optional.
- Bare `prova` is the opportunistic default — "the sensible things present." `prova run <profile>` is
  strict — a *contract* (see Strictness). A profile is a promise about what "green" means; bare
  `prova` is a courtesy.
- The whole tool in one sentence: **`prova run *` drives the quality account; `prova <lane>` /
  `prova evidence` read it; the drivers gate it.**

> **Terminology fix required by this arc:** `prova run <lane>` *today* uses "lane" to mean a
> `[profiles.<name>]` (the help says "run through a named lane ([profiles.<lane>])"). We cannot let
> "lane" mean both a profile and specs/tests/reminders inside the arc built to kill that ambiguity.
> The config already says `[profiles.…]`, so reclaim "lane" for the three media and rename the sugar
> to **`prova run <profile>`**.

## Strict by intent, opportunistic only by default

The audit found strictness split across two axes, and only one is where intuition puts it:

- **Selection axis — already strict, keep it.** `prova run ci` runs *exactly* its lane's tags/proofs;
  a selection (including a lane's baked `tags`) that matches nothing is exit 2 unless `--allow-empty`
  (`main.rs:3300-3365`). This half is already law and correct.
- **Capability axis — opportunistic by default; make named lanes strict.** Today an unmet `requires`
  skips green and an all-skipped run exits 0; the *only* thing that makes a named lane strict on
  capabilities is a hand-written `must_run` (`main.rs:4032-4067`). **Decision (ratified 2026-08-08):
  flip the default — a named lane is strict on the capabilities its tests reference; only bare
  `prova` stays opportunistic.** "A `prova run ci` should run exactly what is specified; only `prova`
  tries whatever is present, and no more."

  Concretely: under a named profile, a capability a selected test `requires` that is unmet is a
  **failure**, not a silent skip — the profile's tests declared they needed it. Bare `prova` keeps
  today's graceful-degradation (skip-green). `must_run` remains, now as the *explicit* form of what
  named lanes get by default, and as the way `[run]` (bare) can opt *into* strictness.

  **Blast radius (this is breaking for consumers):** existing archetype/consumer `prova.toml`s that
  relied on a named lane silently skipping on an under-provisioned box will start failing. Mitigation:
  a per-test/per-lane opt-out (`requires = { "docker", when_absent = "skip" }` or a lane
  `allow_skips = true`) so "this lane genuinely tolerates a missing capability" is *stated*, not
  assumed. Ship with a `prova capabilities` diagnostic (below) so the failure is self-explaining.

## `prova capabilities` — the missing introspection verb

There is a rich capability layer (`Capabilities`, `expr_status`, `builtin_available`, an open
PATH-probe vocabulary — `docker` daemon reachability, `GITHUB_TOKEN`, compiled-in native clients,
any binary on PATH) but **no command to ask what's declared and what's met on this host**. Add one:

```
prova capabilities            # every capability this package references or registers,
                              #   with met/unmet on THIS machine and why
prova capabilities <lane>     # scoped to the capabilities a lane's items require
```

This is a lane-shaped listing in spirit (list + per-item state = met/unmet/too-old/malformed), so it
obeys the same grammar. It is what makes the strict-by-default flip humane: a failed `run ci` points
at `prova capabilities` and the answer is one line.

**Naming hazard — RESOLVED (2026-08-08) by eliminating the collision, not documenting it.** The
package registry used to carry a `capabilities` field (descriptive search tags). Decision: the
registry has no business owning "capabilities" — it was renamed to **`keywords`** (`registry.rs`
`Entry`/`EntryFile`, the search + `info` display + help + `docs/design/registry.md` + the registry
proof fixtures). Now "capability" means exactly one thing across prova — a host fact probed by
`requires`/`must_run`/`prova capabilities` — and a package's discovery metadata is `keywords`, its
host needs still `requires`. Clean cut, no alias (prova's own registry didn't use the field). The
`learn capabilities` topic teaches the single meaning + the `keywords` split.

## Topology lifecycle — unify to one vocabulary

The worst discoverability trap: MCP `{up, down, status}` (warm in-server hold) and CLI
`{up, start, down, ps}` (detached, `running/` records) reuse the same verb names against *different
registries and lifecycles*. `down` resolves against completely different state stores depending on
which surface you're on.

**Decision (ratified 2026-08-08): one lifecycle vocabulary across CLI and MCP.** One set of verbs,
with the foreground/detached/warm distinction made explicit rather than encoded in *which* verb you
picked:

- `prova up <topology>` — foreground (interactive), as today.
- `prova up <topology> --detach` (absorbs today's `start`) — returns; `ps` lists, `down` stops.
- `prova ps` / `prova down` — the one detached-and-warm registry view. MCP renames `status`→`ps` for
  parity and its `up`/`down` operate on the same conceptual registry the CLI reports.
- `watch` stays (dev-loop re-apply).

Unmet-capability policy on `up` also gets reconciled with the rest: today `prova up` *hard-fails*
(exit 2) on an unmet topology `requires` while a test's `requires` *skips* — and the repo's own
`prova.toml` comment claims `up vm` "skips cleanly," which the code contradicts (`main.rs:2109-2127`).
Under the strict-by-default model this is defensible (an explicit `up` is an intent, like a
selection) — so the fix is to make the *code match the doctrine and the comment*: an explicit `up`
is strict (fail), and say so; drop the misleading "skips cleanly" wording.

## MCP parity — close the gaps, make the surfaces one-to-one

Three CLI verbs have **zero MCP reach**: `reminders`, `backlog`, `packages`. An MCP-only agent cannot
see the attention account, the cold shelf, or search/add dependencies. Under the lane model these
fall out for free: the lane-polymorphic `query` tool takes a `lane` argument, so one MCP `query` tool
(or `list` with `lane`) covers `specs`/`tests`/`reminders` at once, and `packages` gets its own tool.

- Every lane is reachable over MCP by the same name the CLI uses.
- MCP `introspect` gained a CLI spelling — *(LANDED 2026-08-08, commit `0bc21`)* `prova introspect
  [<filter>]` reuses `prova_core::help::core_entries`; graduated out of `mcp_tools_are_real_verbs`'
  allowlist (only `status` remains). Completes the CLI discovery duo (`learn` concepts + `introspect`
  shapes), the human/agent-shared surface. v1 is core-only; plugin APIs are a follow-up.
- MCP re-implements the selection *wiring* in `to_selection()` (`mcp.rs:493`); once the engine takes a
  single `Query` value, both front-ends construct the *same* value and the wiring can't drift.

## Alignment proofs — the surfaces are provably one surface

Per the 2026-08-08 direction: **the parity between the query engine, the CLI verb table, and the MCP
tool surface must be proven, not maintained by hand.** These are correspondences between in-process
source tables, so the honest verifier is a **Rust unit test** iterating them directly — the same shape
as the existing `every_verb_resolves_in_learn` and `skill_and_topics_only_name_real_verbs`. But they
are **adopted into prova's own account via the junit verification capability** (`junit.load` +
`DeputedRow`) so `prova evidence`/`owed` speak for them and, to a user, it is just `prova`. This is
the first real payoff of the "unit-test verdicts as first-class proofs" direction terminology.md §2
flagged.

The invariants to encode (each a unit test, wrapped as a deputed proof):

1. **Lane registry is the single source.** There is one `LANES` table; for every lane it holds, there
   exists (a) a `prova <lane>` verb in `VERBS`, (b) an MCP tool reaching it, and (c) a `learn <lane>`
   topic. No lane can exist on one surface and not the others.
2. **Selector parity.** Every axis the CLI selector parser accepts has a matching MCP `SelectionArgs`
   field and vice versa — kills the `to_selection` wiring drift by making a missing field a red test.
3. **Verb↔tool parity, complete.** Every CLI verb that is meant to be on MCP has a same-named tool,
   and the MCP surface proof asserts the **full** tool set. (Today `proofs/mcp/surface_test.lua:93`
   asserts only 8 of 11 — `attest`/`evidence`/`owed` could vanish and stay green. Fix.)
4. **State-filter parity.** Every state-filter sugar verb (`backlog`/`claims`/`promises`/…) resolves
   to a `(lane, state)` that exists in the lane model — a filter can't name a state its lane lacks.
5. **Grammar parity.** The reminders/heed selector and the obligation-family selectors are the *same*
   grammar object as the tests selector (no second `matches_selector`).
6. (**Retained**) verb→`learn` topic resolves for every verb (`every_verb_resolves_in_learn`).

## Increments (each proof-carrying, committed separately)

1. **Lane registry + the first alignment proofs.** *(LANDED 2026-08-08.)* `prova_core::lanes::LANES`
   (specs/tests/reminders, each carrying medium + latent/active state names) is the single source.
   Two unit tests beside `every_verb_resolves_in_learn`: `lane_surface_parity` (invariant 1 — the
   verb and learn-topic legs; the MCP leg is deferred to increment 8 because "tool per lane vs. one
   `query` tool with a `lane` arg" is still open, so gating a tool-named-`<lane>` now would bake in an
   undecided shape) and `mcp_tools_are_real_verbs` (invariant 3, shape-first — every MCP tool maps to
   a real CLI verb; `introspect`/`status` are the known MCP-only names, `status` retiring in increment
   7). Both hold known gaps in an explicit allowlist with a minimality check, so closing a gap FAILS
   until its row is deleted — graduation, like an honored promise. Both proven to fail under mutation;
   clippy `-D warnings` clean. No runtime behavior change. **Deferred out of this increment (surfaced
   during it):** wrapping these unit tests as *deputed proofs in the account* needs a
   cargo-test→junit bridge that does not exist yet — `junit.verify` adopts junit from an arbitrary
   command, but nothing turns prova's own `cargo test` verdicts into junit. `terminology.md §2` already
   flagged this as its own exploration ("Not required… tracked separately"). Tracked below as
   increment 1b; not hand-rolled here.
1b. **Cargo-unit-test junit adoption (the account speaks for the alignment proofs).** Give prova a way
   to emit junit for its own Rust unit tests (an `xtask` step, or nextest's junit output) and a proof
   that `junit.verify`s it, so `prova evidence`/`owed`/`attest` cover the alignment invariants — the
   first real instance of "unit-test verdicts as first-class proofs" (`terminology.md §2`,
   candidate home `docs/design/verifiers.md`). Its own increment because it is a mechanism, not a
   rename; decide the test-runner coupling (nextest vs. a hand-rolled junit emitter) explicitly.
2. **Tests-lane state model.** *(LANDED 2026-08-08 — commit `6a4fa`.)* Discovery now carries per-node
   promise state: `list_plan` returns `Vec<ListNode>{path, promised}`; `discover_suite`/
   `discover_suite_files` follow; `discover_path*` keep `Vec<String>` so the MCP `list` tool and the
   core `discover_path` tests don't ripple. `--proofs` is the mirror selector of `--promises` (settled
   proofs, via a symmetric `apply_specs_filter` + `RunConfig::proofs_only`; mutually exclusive).
   `prova tests` state-tags each node PROMISE/PROOF (internal `--list-tagged`); plain `--list` stays
   bare. Verified: 93 unit tests, clippy `-D warnings`, core discover tests, 144 black-box proofs
   (spec/engine incl. promises/burndown/falsify, mcp, introspection), and a synthetic PROMISE/PROOF
   filter teeth check. **Deferred (was bundled under the old "lane-polymorphic Query" heading):**
   folding the reminders selector (`ledger.rs:74`) and adding selectors to the obligation family into
   one shared grammar — separable, and it rides with increments 4/5.
3. **The lane reporters.** *(LANDED 2026-08-08 — additive half.)* `prova specs` (new — claims +
   backlog side by side, state-tagged, `--claims`/`--backlog` narrow, over `claims::scan`) and
   `prova tests` (new — the tests-lane node listing, delegates to the `--list` path; `--promises`
   narrows). `prova reminders` unchanged. Both verbs registered in `VERBS`, clustered specs/tests/
   reminders in `--help`; the increment-1 `KNOWN_GAPS` verb rows are graduated to empty (the
   minimality check forced it). Verified: 93 bin unit tests green (incl. `lane_surface_parity` now
   passing with all three lane verbs wired), clippy `-D warnings` clean, `backlog` black-box suite
   13/13 through the real harness, and manual smoke of every variant + error path. **Still open:**
   rich per-node state-tagging (PROMISE/PROOF) and a `--proofs` filter on `prova tests` (needs the
   increment-2 `Query` state model); `learn tests` still resolves via the `authoring` alias (wants a
   proper home). *Then the breaking cut:*
3b. **Retire the old verbs.** *(clean trio LANDED 2026-08-08 — commit `84463`.)* `promises`/`burndown`/
   `falsify` removed from `VERBS`; a `RETIRED_VERBS` tombstone redirects each (`prova promises` →
   `prova tests --promises`, exit 2 — not an alias, doesn't dispatch). `promises_subcommand` deleted;
   the `burndown`/`falsify` fns stay as the `prova tests <driver>` bodies. Doctrine rewrite (~20
   mentions across skill.md + 6 topics, guarded by the doc-verb lint) + proof-suite migration (~15
   sites, 6 files) + a tombstone proof. mcp.rs kept under the 1500 gate (tool_router→pub(crate), test
   reads the router directly, `tool_names` helper dropped). Verified: 93 unit tests, clippy, 184
   black-box proofs. **`backlog` retired 2026-08-08 — commit `3ba4f`:** verb removed +
   tombstone → `prova specs --backlog`; `backlog_subcommand` deleted (listing/`--undated`/`promote`
   all covered by `specs`); `prova specs` gained `--undated` and kept the undated-count nudge; docs +
   proofs migrated + a tombstone proof. **Still retiring:** `list` — coupled to the MCP `list` tool,
   so it retires with increment 8.
4. **Drivers as red→green worklists.** *(4a LANDED 2026-08-08 — commit `e1e97`.)* The
   `prova <lane> <driver>` dispatch grammar: `prova specs promote <id>` (rehomed via a shared
   `promote_claim()` that `prova backlog promote` now also calls), `prova tests burndown|falsify`
   (delegate to the run engine), `prova reminders burndown` (= `run --heed`). Additive — the
   top-level `burndown`/`falsify`/`backlog promote` stay until 3b. Verified: 93 unit tests, clippy,
   backlog proofs 13/13 (factored promote), end-to-end smoke incl. a specs-promote happy path.
   **4b LANDED 2026-08-08 — commit `ca901`:** `prova specs backfill` — the reverse-`owed` coverage
   gate. `ListNode` gained `backed` (leaf has non-empty `covers`); `--backfill` lists every proof no
   claim backs and gates (exit 1 until all backed, 0 when complete). Read-only (skips IDE wiring +
   run state), and it NEVER fabricates a stub — proven by `backfill_test.lua`'s writes-nothing test.
   Taught in `prova learn claims`. Verified: 93 unit tests, clippy, spec/engine proofs, file_size +
   terseness gates.
5. **Selectors on the cross-lane account.** `owed`/`attest`/`evidence` take the shared grammar;
   reconcile the owed-vs-evidence DUE-reminder discrepancy; settle evidence(reporter) vs
   owed/attest(drivers) — the parked question, now with the red→green lens.
5b. **`prova run <profile>` (lane→profile rename) + profiles as gate compositions.** Reclaim "lane"
   for the media; make a profile compose deputed gates (junit/exit-code/ratchet/heed), not just proof
   selection. Prereq for the run vision; couples to increment 1b.
5. **Strict-by-default capabilities** + the `when_absent`/`allow_skips` opt-out + migration note for
   consumers. Breaking; its own increment because it touches consumer `prova.toml`s.
6. **`prova capabilities`.** *(v1 LANDED 2026-08-08 — commit `4f661`.)* Reports prova's built-in
   capability vocabulary (docker/github/OS/network + compiled natives) with each one's host status
   (MET/UNMET + reason). A reporter, exit 0. `engine::builtin_capability_names()` single-sources the
   list (drift-guarded by a unit test vs. `is_builtin_capability`); `Topic::Capabilities` +
   `topics/capabilities.md` teach requires(skip)/must_run(fail) and disambiguate from the registry's
   advertised `capabilities`. Proof: `capabilities_test.lua` (4). **v2 (needs the run context to load
   `prova.lua` registrations):** fold in THIS package's declared `must_run`/topology-`requires`/
   per-test `requires` with status — hook `resolve_from_manifest` after the capability load, before
   the `must_run` gate (so it reports unmet rather than failing on it).
7. **Topology lifecycle unification** (`up --detach` absorbs `start`; MCP `status`→`ps`; `up`
   strictness matches doctrine and the comment). Its own increment — largest infra surface.
8. **MCP parity sweep.** One `query` tool with `lane`; `packages` tool; `introspect` CLI spelling or
   retirement; invariants (2) and (3), with the surface proof asserting the full set.

## Settled naming decisions (all ratified 2026-08-08)

- **Lanes are the nouns; states are `--flags`.** `prova <lane> [--state]`. Three lanes:
  specs / tests / reminders. No conflation of state with lane.
- **The executable lane's verb is `tests`.** Tests are the medium for promises and proofs, exactly as
  specs are the medium for backlog and claim — the medium-naming rule applies without exception.
  `proof` stays the done-state within the lane. (The "prova's pitch is *prove*, not *test*" tension
  was weighed and does not override the rule.)
- **No `query` verb, no `list` verb.** `query` is ceremony; `list` begs "list what?". The
  lane-polymorphic query is the *engine* beneath the lane verbs, never a typed word. `list` is
  removed, not aliased.
- **No convenience aliases.** `backlog`/`promises`/etc. do not survive as verbs — a state is an
  adjective on its lane. Clean pre-1.0 cut.
- **Drivers are `prova <lane> <driver>`, unified as red→green worklists** with a `gate`/`xfail` red
  policy (see *Drivers*). `burndown`/`falsify` are tests-lane drivers over the run engine, not
  top-level verbs or bare run-flags; `promote`/`backfill` are specs drivers.
- **`prova run <profile>`** — "lane" reclaimed for the media; the profile-runner says "profile".
  Profiles are gate compositions (may depute external verifiers).

## Open sub-decisions

- **Cross-lane account shape** (`evidence` vs `owed` vs `attest`): reporter-vs-driver split is chosen,
  but whether the three collapse (`evidence [--owed] [--gate]`) or stay three verbs is deferred to
  increment 5 — they already share one reconciliation engine, so it is a surface decision, not an
  engine one.
- **`prova run` built-in target names vs pure flags** for the driver delegations (whether burndown is
  *only* `prova tests burndown` or also reachable as a run target). Decide alongside increment 5b.
