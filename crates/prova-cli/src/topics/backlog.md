# backlog — capture work in place without owing it

A bug surfaces mid-task. A section of a spec is worth shaping but not now. You do not want to lose
it, and you do not want to *owe* it yet — an obligation admitted the moment you notice it distracts
the very work that noticed it. A `<!-- backlog: id -->` anchor is the answer: it captures the thing,
in the document where it belongs, in a state that is deliberately muted.

```markdown
<!-- backlog: flaky-teardown -->
Teardown occasionally leaves a container behind — investigate, someday.
```

Backlog and claim are the **two states of one prose obligation** — same anchor shape, one keyword
apart, one shared id namespace. `claim` is owed: it is reconciled, a proof can bind it, and an
uncovered one reports as `UNPROVEN`. `backlog` is the cold state of the very same thing:

- it never appears in `prova owed`,
- it never fails `prova attest` (the CI gate),
- a bare `prova` run does not read it at all.

So a backlog item can sit *inside a doc that is being actively driven* without adding one thing to
what that doc owes right now. That muting is the whole point.

## The one write: promote

Backlog is human-driven — nothing in the runtime pushes a cold item across the line; you do, when
its time comes. Promotion is a **keyword flip in place**: the id and its prose do not move, only the
state changes.

```bash
prova backlog                       # the cold shelf — every backlog item, muted from `owed`
prova backlog promote flaky-teardown  # thaw it into a claim; the burndown sees it now, not before
```

After promoting, `<!-- backlog: flaky-teardown -->` becomes `<!-- claim: flaky-teardown -->` on its
own line — so the diff reads as exactly *this became active*, and the address a proof will name
(`docs/design.md#flaky-teardown`) is already the one the reader sees. Discharge it the ordinary way:
a proof with `covers = "docs/design.md#flaky-teardown"`.

Demotion (claim → backlog) is not a keyword flip you should reach for blindly: cooling a claim back
is only safe when nothing binds it, and that check needs the proofs in hand. Do it by editing the
anchor when you know the claim is unbound.

## Only a claim can be bound

The invariant that keeps the two states legible: a proof may only `covers` a **claim**. Point one at
a backlog item and the ledger says so rather than pretending the cold item is discharged:

```
BACKLOGGED  docs/design.md#not-ready
            contract_test › … covers it, but it is still a backlog item —
            `prova backlog promote not-ready` to make it a claim a proof can discharge
```

## Opting in

Backlog shares the spec scan roots: `[specs] docs = ["docs/design", "README.md"]` in `prova.toml`.
**Absent by default** — no section, no scan, no cost. Declaring `docs` opts in both states at once,
because they live in the same files.

## Priming a burndown, and draw-down schemes

The intended rhythm: burn down to green, then open the backlog, pick a theme (backlog items in one
file are one `prova backlog` away from being read together), and `promote` the few you are ready to
take on. Now the next burndown is staged — and nothing was owed until you chose it.

Backlog is data and a query; it enforces nothing on its own. Enforcement — *draw a backlog item down
by a deadline*, *don't let this doc's backlog grow past N* — is a **reminder** watching a condition,
elevated to a gate with `--heed` when you're ready (`prova learn reminders`). Reminders are the
primitive for cooking up draw-down schemes on top of the backlog; the backlog stays a plain,
merge-friendly, promotable list.

See also: `prova learn claims` (the owed state and its ledger), `prova learn promises` (the
executable spec — a contract you *are* committing to prove, just not yet).
