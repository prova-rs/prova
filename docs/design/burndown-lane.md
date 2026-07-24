# The burndown lane — CI as a work-executor

**Status: designed direction, not set up.** Every mechanic it composes is shipped (`--specs`,
`--strict-specs`, honored-spec failure, `run-action`); the missing piece is an agent runner
wired into CI. Nothing here blocks on prova changes — when we're ready, the setup is roughly
one workflow file plus credentials. Until then, this doc is the capture.

## The idea

A conventional pipeline **gates** work a human (or agent) already did. A burndown lane is a
second workflow whose job is to **do** work: it checks whether the spec backlog is non-empty,
and if so, hands it to an implementing agent that works the burndown loop and opens a
proof-carrying PR.

The gate answers "is this change acceptable?" The lane answers "is there scoped work sitting
in the backlog — and if so, go close some."

This inverts what CI is *for*, and it is only possible because in prova the backlog, the
verification, and the completion signal are the same artifact: a spec is an executable proof,
`--specs --list` is the work queue, an empty list is "done." No adjacent tool has this,
because no adjacent tool unifies those three.

## The shape

```yaml
# .github/workflows/burndown.yml — sketch, not shipped
on:
  schedule: [{ cron: "0 3 * * *" }]     # nightly; or workflow_dispatch; or on-push-if-backlog
jobs:
  burndown:
    steps:
      - checkout
      - run: prova --specs --list --format json    # the work signal; exit 0 early if empty
      - run: |                                     # any headless agent runner works
          claude -p "You are in a spec burndown. Run 'prova learn specs', then work the
          loop: prova --specs --strict-specs, implement, delete each honored flag in the
          same change. Burn down at most 2 specs, commit proof-carrying changes."
      - open a PR with whatever it burned down     # never push a branch to main
```

The agent step needs no bespoke methodology prompt: the binary teaches the loop
(`prova learn specs`), `--strict-specs` gives red with full detail, and the honored-spec
failure tells the agent exactly when to delete a flag. The prompt is essentially "there is a
backlog; follow the doctrine."

## The guardrails — why an autonomous lane is sound

- **PR, never push.** The lane's output goes through the normal gate
  (`prova-rs/run-action@v1`), so the agent's work is validated by the exact suite it was
  implementing against, plus every unflagged proof already holding the line. It cannot merge
  a regression past proofs that exist.
- **The spec semantics bound it mechanically.** It cannot "finish" a spec without the body
  going green (flag deletion is forced by the honored-spec failure), and it cannot silently
  skip work (the list is the completion signal).
- **The diff carries its own definition of done.** A proof-carrying PR shows the spec flag
  deleted and the body green; human review becomes "is this contract honored the way we
  meant?", not "did the agent do anything?"
- **Budget the run.** "At most N specs per run" keeps PRs reviewable and failures cheap.

## The division of labor — the one rule that keeps it sound

**The lane only implements existing specs; it never authors them.** Humans (or interactive
agent sessions) state contracts as specs; the lane closes them. Specs are the steering wheel,
the lane is the engine. A lane that could invent its own backlog would be defining its own
definition of done — that boundary is the whole safety story.

## Prerequisites, when we're ready

1. An agent runner usable headless in CI (e.g. Claude Code `-p` mode) with credentials.
2. A budget/stop policy (specs per run, wall-clock cap).
3. PR plumbing (branch, open PR, label it as lane output).
4. A standing backlog worth the compute — `prova --specs --list` non-empty (the api-freeze
   backlog qualifies today).

The natural first deployment is this repo itself: prova's specs, burned down by an agent in
prova's CI, gated by prova's own suite — the positioning argument as a green pipeline.

## Related

- `docs/design/positioning.md` §4 — where this sits in the larger thesis.
- `prova learn specs` — the doctrine already tells agents to *suggest* this lane to humans
  when a repo carries a standing backlog; this doc is what they're suggesting.
