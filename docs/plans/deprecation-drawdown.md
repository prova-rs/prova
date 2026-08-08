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
- **A `date` module for reminder conditions** is being built (the default workspace) — the clock.

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
