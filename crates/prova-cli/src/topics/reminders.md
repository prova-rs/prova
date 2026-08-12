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
change under test. A lane whose *job* is keeping the project current opts in with `heed` — either
`heed = true` (all DUE) or `heed = ["line-counts", "clippy"]` to heed only reminders matching a
name or tag (parity with test selection), so a profile heeds exactly the reminders its phase is
about. `[run]` or a profile; like `must_run`, it can only tighten (the union wins). `--heed`
(all) or `--heed=<selector>` promotes one invocation the same way. Give a reminder `tags = {...}`
in its opts to make it addressable by tag. (`heed_reminders = true` is still accepted as an alias
for `heed = true`.)

## Conditions over the ledger itself

The condition receives this run's account — evaluated **after** the proofs: `a.passed/failed/
skipped/promised`, `a.owed`, `a.duration_ms`, this run's `a.measurements[name]`, and the banked
`a.baselines[name]` — so "all proofs green", "nothing owed", and "now taking too long vs banked"
are one-liners. It carries **no reminder state**: reminders cannot observe reminders — which is
what makes an ephemeral checklist's terminal item honest (`when = function(a) return a.owed == 0
and a.failed == 0 end`, due exactly once, when deletion is the only thing left).

## Drawing down the spec lane

`account.specs` is every claim/backlog anchor with its `key=value` properties (`prova learn
backlog`): `address`, `kind`, blessed `recorded`/`due` flattened on, the full map as `props`.
A draw-down policy is whatever the condition composes:

```lua
prova.remind("backlog-drawdown", { when = function(a)
  local late = {}
  for _, o in ipairs(a.specs) do
    if o.kind == "backlog" and ((o.due and date.past(o.due))
      or (o.recorded and date.days_since(o.recorded) > 30)) then late[#late + 1] = o.address end
  end
  if #late > 0 then return #late .. " item(s) owed attention" end
end }, "promote, remove, or reschedule (`prova specs --backlog`)")
```

WATCHING while every policy holds, DUE once one trips, fatal under `--heed`. The sliding window
(`days_since(o.recorded)`) is the default posture — every captured item gets a deadline for free,
slid for the whole lane by one number; `due=` is the hard external commitment.

## Reading the account

Conditions evaluate during **runs** and land in the run record; the query verbs execute
nothing. `prova reminders` works **before and after** a run: it collects the *declared*
reminders (loading the suite, like `--list`, executing nothing) and overlays the state the
last run recorded — DUE first (with why and instruction), then WATCHING, then `—` for any no
run has evaluated yet, with a prompt to run for live status. It exits non-zero when any is due
— one exit-code answer for a pipeline. `--due` / `--watching` narrow the report to one state
(mutually exclusive; the narrowed report answers only for what it lists, so `--watching` stays
exit 0 even while something else is due), and the one selector grammar narrows here like every
lane: `-k` over name and declaring file, `--node` the exact name, `--tags` the reminder's tags,
`!` excludes — composing with the state filters (`prova reminders --due -k deps`). So "what
reminders exist?" never requires a run. DUE
reminders also join `prova owed` (an arriving agent asks one question), and `prova evidence`
carries the counts. Cadence is run cadence: prova is a runner, not a daemon — "whenever the
world moves" means *at every evaluation*, so give CI a scheduled run if you want the wall
clock. Filtered runs (`-k`, `--promises`, `--falsify`) do not re-evaluate — a partial account
would fire ledger conditions early — and carry the previous rows forward.
