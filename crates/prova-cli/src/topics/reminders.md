# reminders — obligations the world creates

A **proof guards the past; a reminder watches the future.** `prova.remind` states an obligation
whose *trigger* is a condition of the world and whose *discharge* is an act — project
management, not specification. Where a promise is an executable spec an agent implements, a
reminder is a tripwire: WATCHING while the world holds still, **DUE** the moment it moves — an
upstream cuts the release you wait on, a dependency drifts past your pin, a checklist's last
real item graduates.

```lua
prova.remind("stay current with prova-redis", {
  when = function(account)
    local latest = latest_tag("prova-rs/prova-redis")   -- plain Lua over shell/http/fs
    -- Falsy = watching. Truthy = due; a string becomes the report's "why".
    return semver_gt(latest, PINNED) and (latest .. " is out; we pin " .. PINNED)
  end,
}, "integrate the new release and bump [dependencies]")
```

- The **message is the instruction** — what to DO when it fires, because the discharge is an
  act, not an assertion. Nothing about a reminder graduates.
- The **standing** form (compare two observables, as above) silences itself when you act. The
  **one-shot** form ("v1 exists") stays due until you act and delete or rewrite it. One rule
  covers both: a reminder is due whenever its condition holds. No latching, no event log.
- `requires = { "network" }` gates evaluation like a test's — but unmet means **UNEVALUATED**,
  never WATCHING: a watcher that could not look must stay visibly disarmed.

## Two accounts, strictly separated

A reminder **looks like a test and is not one**: it never enters the tally, the selection,
`--promises`, or `burndown` (attention is not implementable work). The run headline stays the
evidence account; fired reminders add their own `N reminders due` section, and a WATCHING
reminder is silence. **DUE never fails a run by default** — the world moving is not a defect in the
change under test. A lane whose *job* is keeping the project current opts in with
`heed_reminders = true` (`[run]` or a profile; like `must_run`, it can only tighten), or
`--heed` for one invocation.

## Conditions over the ledger itself

The condition receives this run's account — evaluated **after** the proofs, so "all proofs
green" / "nothing owed" are one-liners: `account.passed/failed/skipped/promised`, plus
`account.owed` (the ledger's remainder, as `prova owed` counts it). It carries **no reminder
state**: reminders cannot observe reminders. This is what makes an ephemeral checklist's
terminal item honest — `when = function(a) return a.owed == 0 and a.failed == 0 end`, watching
while work remains, due exactly once, when deletion is the only thing left.

## Reading the account

Conditions evaluate during **runs** and land in the run record; the query verbs execute
nothing. `prova reminders` lists every reminder with its state (DUE first, with why and
instruction) and exits non-zero when any is due — one exit-code answer for a pipeline. DUE
reminders also join `prova owed` (an arriving agent asks one question), and `prova evidence`
carries the counts. Cadence is run cadence: prova is a runner, not a daemon — "whenever the
world moves" means *at every evaluation*, so give CI a scheduled run if you want the wall
clock. Filtered runs (`-k`, `--promises`, `--falsify`) do not re-evaluate — a partial account
would fire ledger conditions early — and carry the previous rows forward.
