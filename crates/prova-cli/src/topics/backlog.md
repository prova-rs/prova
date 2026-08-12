# backlog — capture work in place without owing it

A bug surfaces mid-task, worth keeping but not worth *owing* yet. A `<!-- backlog: id -->` anchor
captures it, in the document where it belongs, in a state that is deliberately muted.

```markdown
<!-- backlog: flaky-teardown -->
Teardown occasionally leaves a container behind — investigate, someday.
```

Hand-write it, or let `prova specs capture <id> "<prose>" --file <doc>` (MCP: `capture`) do the
verified write: it refuses unscanned paths and duplicate ids, stamps `recorded=`, and rescans.

Backlog and claim are the **two states of one prose obligation** — same anchor shape, one keyword
apart, one shared id namespace. `claim` is owed: it is reconciled, a proof can bind it, and an
uncovered one reports as `UNPROVEN`. `backlog` is the cold state of the very same thing:

- it never appears in `prova owed`,
- it never fails `prova attest` (the CI gate),
- a bare `prova` run does not read it at all.

So a backlog item sits *inside a doc that is being actively driven* without adding one thing to
what that doc owes right now — the muting is the whole point.

## The one write: promote

Backlog is human-driven — nothing in the runtime pushes a cold item across the line; you do, when
its time comes. Promotion is a **keyword flip in place**: the id and its prose do not move, only the
state changes.

```bash
prova specs --backlog                  # the cold shelf — every backlog item, muted from `owed`
prova specs promote flaky-teardown     # thaw it into a claim; the burndown sees it now, not before
```

After promoting, the anchor's keyword reads `claim` in place — the diff reads as exactly *this
became active*, and the address a proof will name (`docs/design.md#flaky-teardown`) is already the
one the reader sees. Discharge it the ordinary way: `covers = "docs/design.md#flaky-teardown"`.

Demotion (claim → backlog) is deliberately not a verb: cooling a claim back is only safe when
nothing binds it, and that check needs the proofs in hand. Edit the anchor when you know it is unbound.

## Properties — named, optional, composable

After the id: `key=value` properties. Blessed and ISO-validated: `recorded=` (when the item was
written down — the capture verbs stamp it; the ideal on every anchor) and `due=` (a hard
external deadline). Any other key is yours, passed through verbatim:

```markdown
<!-- backlog: flaky-teardown recorded=2026-08-11 due=2026-09-01 owner=jimmie -->
```

**Stamp `recorded`**: reminders compose draw-down policy over `account.specs`
(`days_since(o.recorded) > 30`, `date.past(o.due)` — `prova learn reminders`). A bare date after
the id is an error; `--undated` lists the unstamped; properties survive promotion untouched.

## Only a claim can be bound

The invariant that keeps the two states legible: a proof may only `covers` a **claim**. Point one at
a backlog item and the ledger says so rather than pretending the cold item is discharged:

```
BACKLOGGED  docs/design.md#not-ready
            contract_test › … covers it, but it is still a backlog item —
            `prova specs promote not-ready` to make it a claim a proof can discharge
```

## Opting in

Backlog shares the spec sources — declare them under `[specs]` (`prova learn spec`). **Absent by
default** — no section, no scan, no cost. One source opts in both states: they live in the same files.

## Priming a burndown, and draw-down schemes

The intended rhythm: burn down to green, then open the backlog, pick a theme (backlog items in one
file are one `prova specs --backlog` away from being read together), and `promote` the few you are ready to
take on. Now the next burndown is staged — and nothing was owed until you chose it. A green
`prova owed` prompts this itself: with a non-empty shelf it appends a one-line breadcrumb (a
count and the query, never the items) — announced exactly when you would go shopping, muted
the rest of the time.

Backlog is data and a query; it enforces nothing on its own. Enforcement — *draw this item down by
a deadline*, *don't let this doc's backlog grow past N* — is a **reminder** watching a condition,
elevated to a gate with `--heed` when you're ready (`prova learn reminders`); the backlog stays a
plain, merge-friendly, promotable list.

**Where does it go?** Placement is the package's fact, not a guess: `prova learn project` names
the writable spec sources and any house rules its context carries — write the anchor in the doc
whose subject owns the item, never where the ledger does not scan. See also: `prova learn claims`
(the owed state), `prova learn promises` (the executable spec, committed but not yet proven).
