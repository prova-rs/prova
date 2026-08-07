# Backlog — the cold state of a claim

Status: **core shipped** (this change), in the `backlog` workspace. Follow-ups listed below.

## The idea

Lanes in prova are two-state dualities, each an optional link that locks into place as you opt in:
`backlog → claim → promise → proof`, with `reminder → heed` an orthogonal rail. Backlog is the new,
coldest tier — **the cold state of a claim**.

A claim is owed the moment you anchor it. But mid-task you often want to *capture* something without
*owing* it: a bug you don't want to chase now, a spec section worth shaping but not this session,
work another developer sketched in a doc you aren't ready to process. Admitting an obligation the
instant you notice it distracts the work that noticed it. Backlog is capture without obligation, **in
place** — no separate backlog directory, no shuffling files out of the prova lifecycle.

```markdown
<!-- backlog: flaky-teardown -->      <!-- claim: flaky-teardown -->   (after promotion)
Teardown occasionally leaves a container behind.
```

## What shipped

- **One artifact, two states.** `prova_core::ledger::claims::Kind { Claim, Backlog }` on the scanned
  anchor. `<!-- backlog: id -->` parses exactly like `<!-- claim: id -->`; one shared id namespace
  (the per-file duplicate check spans both states), so promotion is a keyword flip and a stray
  duplicate across states is still caught.
- **Muting, everywhere it could leak.** Backlog items are filtered out of `owed` (`reconcile`), the
  `evidence` account (`account`), and the CI gate (`attest_all`). A bare `prova` run never reads
  them. Muting in one place and not another would be a trap; all four paths agree.
- **Only a claim can be bound.** A proof whose `covers` resolves to a backlog anchor reports
  `BACKLOGGED` (new `Status`), with the one-keyword remedy — never silently discharged. This is the
  invariant that keeps the two states from collapsing.
- **`prova backlog`** — query-only listing of the cold shelf. **`prova backlog promote <id>`** — the
  one write: flips the keyword in place (`ledger::claims::promote`), preserving the id, the prose,
  and every other byte of the doc. Demotion (claim → backlog) is intentionally *not* a CLI verb: it
  is only safe when nothing binds the claim, a check that needs the proofs in hand.
- **Discovery.** `prova learn backlog` topic; catalog entry; `park`/`icebox`/`defer`/`shelf`/
  `promote` aliases. Reciprocal cross-link from `prova learn claims`.
- **Proofs.** `proofs/spec/engine/backlog_test.lua` (8) — muting from owed, listing, in-place
  promotion, `BACKLOGGED`, the CI gate not failing on a parked item, bare-run inertness, the shared
  namespace, and discoverability. Plus unit tests on `parse_anchor` for both keywords.

## Design decisions

- **Backlog reuses `[claims] docs`.** The two states live in the same files, so they share the scan
  roots. The section name `[claims]` now slightly under-describes its contents ("prose obligation
  docs" would be truer); not worth a breaking rename today. Noted.
- **Human-driven, no forcing function.** Unlike promise→proof (a promise *fails* when its body goes
  green) or reminder→heed, nothing pushes a backlog item across the line. That is by design: backlog
  is the one lane where rot is the expected steady state, so its value is entirely in query +
  promote ergonomics, not runtime pressure. Enforcement (draw-down schemes, "don't let this doc's
  backlog grow past N", "drain this item by a date") is a **reminder** watching a condition,
  elevated to a gate with `--heed` — reminders are the primitive for cooking up draw-down schemes.

## Open follow-ups

1. **Terminology scrub (decision needed).** Before backlog, the word "backlog" already meant
   *promises* — "the executable backlog" — across `skill.md`, `topics/{promises,running,pdd}.md`,
   `mcp.rs`, the `learn.rs` spec-first slot, and `docs/design/{positioning,burndown-lane,README,
   lifecycle}.md`. The new lane claims the term; the old usage should be re-termed (recommend
   *"the executable spec"* / *"the open spec surface"*). The new surface (this change) is already
   internally consistent — `backlog.md` says "executable spec" when pointing at promises. The old
   prose is left untouched pending a wording call, since it is woven into the north-star docs.
2. **Reverse-query-as-reminder recipe.** Ship a documented `prova.remind` condition for
   *"every promise/proof has a backing claim"* (the mirror of `owed`: proof → claim, documentation
   coverage). Off by default; a nudge that elevates to a gate via `--heed`. Needs the explicit
   backing link (`covers`) it already has.
3. **MCP `backlog` tool** — parity with the CLI verb for the agentic harness / substrate UI, which
   will filter by lane (claims / promises / proofs / reminders / backlog).
4. **Substrate UI toggle contract.** The UI flips backlog ↔ claim in place; promotion is always
   legal, demotion only when the claim is unbound. The unbound check belongs to whatever performs
   the demote (it has the proofs); the CLI deliberately omits demote for that reason.
