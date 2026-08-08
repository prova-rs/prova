# Warnings with a half-life — draw-down reminders over deprecations

Status: **design idea** (Jimmie, mid-build on the `[specs] docs` deprecation). A warning should not
stay a warning forever. From its first sighting, a clock starts; a reminder holds it soft through a
grace window, then goes DUE, and with `--heed` fatal. The deprecation must be drawn down by the
deadline or CI goes red. This is "reminders as the draw-down primitive" turned on prova's OWN
(and eventually the user's) warnings.

## Why it fits right now

Three pieces just converged:
- **Deprecation warnings exist** (`[specs] docs` → `[[specs.source]]`, this branch).
- **Reminders are the draw-down primitive** (`prova.remind` + `--heed`, soft→hard).
- **A `date` module for reminder conditions** — UNBLOCKED. `crates/prova-core/src/modules/date.rs`
  (commit a25ba, "ergonomic time helpers over os.* for reminder conditions") is an ANCESTOR of this
  backlog line (the two were rebased linear, not diverged), so it is already in-tree. The clock is here.

## One mechanism, two obligation kinds (unifies items 3 and 5)

Draw-down is the same shape whether the deadline is computed or authored:

| obligation | id | deadline | source |
|---|---|---|---|
| deprecation warning (item 3) | warning id (`specs-docs-deprecated`) | `first_seen + grace` (computed) | `.prova/deprecations.toml` (stamped) |
| dated backlog/claim (item 5) | anchor id | the `YYYY-MM-DD` on the anchor (authored) | the doc anchor itself |

Both feed ONE reminder condition: read the dated obligations, compare `deadline` to `now` (the `date`
module) → WATCHING before, DUE after, `--heed` fatal. Build the condition once; both kinds plug in.

### Item 5 — dated anchors

Claims and backlog items carry an optional `YYYY-MM-DD` so a reminder can draw them down:

```markdown
<!-- backlog: flaky-teardown 2026-09-01 -->   (open: bare ISO date vs. keyed `due=`/`by=`)
```

Parser change: the anchor accepts an optional date token after the id (today it is id-only). The date
lands on `Claim` (both kinds), and `prova backlog`/`owed` can show/sort by it. An authored due-date is
the explicit sibling of the deprecation's computed one — same reminder condition draws both down.

## The load-bearing piece: "from the start of the first warning" needs committed state

A deadline relative to *first sighting* means prova must remember when each warning was first seen —
which requires two things it does not have yet:

1. **Warnings need identity.** Today the deprecation is a bare string. To track first-seen, each
   deprecation needs a stable id (`specs-docs-deprecated`), the same move claims and reminders
   already made. Warnings become first-class, identified events (not just stderr text).
2. **A committed first-seen ledger.** `first_seen` must be shared across the team/CI, not
   per-machine — so a committed file (`.prova/deprecations.toml`: id → first-seen date), analogous
   to how `.prova/baselines/` is committed. First run that sees a warning stamps the date; later
   runs read it.

The reminder condition then: read the ledger, `deadline = first_seen + grace`, compare to `now`
(the `date` module) → WATCHING before, DUE after. `--heed` makes DUE fatal. That escalation IS the
draw-down.

## Open decisions

- **Who sets the grace window?** Per-deprecation (prova declares "retires in N days / 2 releases")
  vs. per-project (the consumer's tolerance). Likely both: prova suggests, the project can tighten.
- **Opt-in, to stay faithful to the explicitness stance.** prova ships the built-in condition; you
  add the reminder if you want warnings to have teeth. Not automatic.
- **Generality.** Not just prova's own deprecations — any warning prova observes (a linter, a
  verifier, a driver) could carry an id and a draw-down. The ledger + condition are the reusable
  parts.
- **Stamp-on-write ordering.** The first run that emits a warning must also record its first-seen
  date atomically, or a warning seen only in read-only/CI contexts never starts its clock.

## Sequencing

After the terminology phase. Depends on the `date` module landing (default workspace). The minimal
first cut: give the `[specs] docs` deprecation an id, stamp `.prova/deprecations.toml`, and ship one
built-in reminder condition (`deprecations`) — dogfooding the pattern on prova's own first warning.
