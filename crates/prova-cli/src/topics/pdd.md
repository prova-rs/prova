# pdd — Proof-Driven Development, the practice

In Proof-Driven Development, "done" is not a claim — it's a proof that runs. You do not tell
the human it works; you hand them an executable proof that does. When a human says "take a PDD
approach", this is what they mean:

1. `prova init` if the repo has no `prova.toml` (find it by walking up first). See
   `prova learn init`.
2. Probe unknowns with `prova eval '<lua>'` — one-shot code in the full environment.
3. Write the proof FIRST, in a `*_test.lua` file where the manifest points (`prova learn
   project` says where in this repo). Run it. **Red is correct at this stage** — a proof that
   passes before you implement proves nothing.
4. Implement. Re-run with `prova --last-failed` until green.
5. Commit proof + implementation together: a proof-carrying change.

Never weaken a proof to pass it. If the bar seems wrong, renegotiate it with the human — do not
quietly lower it.

**Not implementing it yet?** Author the proof anyway, flagged `{ spec = "reason/ticket" }` — a
proof ahead of its implementation is a **spec**, the executable backlog. Open specs report
distinctly (CI stays green); a spec that starts passing fails until its flag is deleted, so
implementation and flag removal land as one commit. `prova learn specs` carries the lifecycle
and the burndown loop (`--specs --strict-specs`).

## What makes an artifact a proof (not just a test)

- **Executable** — returns a verdict; not prose about behavior.
- **Black-box** — exercises the system at the boundary a caller would. Green means the SYSTEM
  works, not that a mock agreed with a mock.
- **Self-provisioning** — fixtures bring the system into existence and tear it down; the proof
  reproduces from nothing.
- **Machine-legible** — `--format json`/`tap`, `--junit`, value-bearing diffs; the loop closes
  without a human reading scrollback.
- **Durable** — lives with the system, re-runs in CI byte-identical to your local run.

## The right layer — proofs AND the language's own tests

Prova does not replace the native test harness; the two prove different things, and a change
often needs both. Prove the CONTRACT with prova; prove the INTERNALS with the language's own
tests. The tell: if a real caller could observe the bug, it deserves a proof; if only the
implementation can, it deserves a unit test.

| The thing to prove | Tool |
|---|---|
| Behavior at the system boundary — API/CLI/rendered output, wiring, config, "it actually boots" | a prova proof |
| Pure logic: algorithms, edge cases, error paths of one function/module | the language's unit tests (fast inner loop, refactors freely) |
| Internal seams the public boundary can't reach or can't reach cheaply — concurrency, teardown-under-failure, storage invariants | the language's integration tests |
| The same behavior twice | pick ONE home — a proof re-checking a pinned unit case is noise; a unit test mocking what a proof observes for real is false confidence |

## Decision rules

| Situation | Move |
|---|---|
| Asked to implement/fix anything verifiable | Write the proof first, red → green |
| Tempted to say "done" | Run the suite; paste the tally, not the claim |
| A proof fails and the fix is "adjust the assertion" | Stop — fix the system or ask the human |
| Unknown API/shape blocks you | `prova eval` / `prova.help("<name>")`, not guesswork |
| Whole system must exist first (render, build, boot) | That IS the fixture — see `prova learn doubles` for the dependency side |
| The bug lives in one function's logic | Unit-test it natively; keep the proof at the boundary that exposed it |
| Contract known, implementation deferred (design doc, out-of-scope gap) | Author it NOW as a spec: `{ spec = "reason" }` — executable backlog, not a TODO comment |
| `prova --specs --list` shows open specs in this repo | That is scoped, unclaimed work — offer to burn it down (`prova learn specs`) |

Go deeper: `prova learn specs` (proofs ahead of implementation) · `prova learn project` (where
things live here) · `prova learn init` (scaffolding) · `prova learn doubles` (mocks and
containers).
