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

## What makes an artifact a proof (not just a test)

- **Executable** — returns a verdict; not prose about behavior.
- **Black-box** — exercises the system at the boundary a caller would. Green means the SYSTEM
  works, not that a mock agreed with a mock.
- **Self-provisioning** — fixtures bring the system into existence and tear it down; the proof
  reproduces from nothing.
- **Machine-legible** — `--format json`/`tap`, `--junit`, value-bearing diffs; the loop closes
  without a human reading scrollback.
- **Durable** — lives with the system, re-runs in CI byte-identical to your local run.

## Decision rules

| Situation | Move |
|---|---|
| Asked to implement/fix anything verifiable | Write the proof first, red → green |
| Tempted to say "done" | Run the suite; paste the tally, not the claim |
| A proof fails and the fix is "adjust the assertion" | Stop — fix the system or ask the human |
| Unknown API/shape blocks you | `prova eval` / `prova.help("<name>")`, not guesswork |
| Whole system must exist first (render, build, boot) | That IS the fixture — see `prova learn doubles` for the dependency side |

Go deeper: `prova learn project` (where things live here) · `prova learn init` (scaffolding) ·
`prova learn doubles` (mocks and containers).
