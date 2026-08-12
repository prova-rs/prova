--- prova's own quality gates, as reminders — the attention account prova holds ITSELF to.
---
--- Distinct from the proofs, which are the evidence account (what prova DOES). A reminder is a
--- standing condition the world can trip: it reports (WATCHING → DUE) and a context elevates it to
--- fatal with `--heed` (docs/design/reminders.md). This file is where prova dogfoods its own gates;
--- add one as it earns its place. Reminders here are evaluated on every full run and reported by
--- `prova reminders`.

--- Draw down prova's own backlog, composed over anchor properties
--- (docs/design/lifecycle.md#anchor-records-when-it-was-captured). Two policies in one watcher:
--- a `due=` past its date is a commitment broken (most immediately the deprecation bridges in
--- docs/design/deprecations.md), and a `recorded=` older than the window is a shelf rotting —
--- the sliding deadline every item gets for free from its capture stamp, moved for the whole
--- lane by changing one number here. WATCHING while there is time; DUE, with the addresses
--- named, once a policy trips. It never forces the work owed — a human still promotes or
--- reschedules — but it cannot rot unseen.
prova.remind("backlog-drawdown", {
  when = function(a)
    local late = {}
    for _, o in ipairs(a.specs) do
      if o.kind == "backlog" then
        if o.due and date.past(o.due) then
          late[#late + 1] = o.address .. " (due " .. o.due .. ")"
        elseif o.recorded and date.days_since(o.recorded) > 60 then
          late[#late + 1] = o.address .. " (on the shelf " .. date.days_since(o.recorded) .. " days)"
        end
      end
    end
    if #late > 0 then
      return #late .. " backlog item(s) owed attention: " .. table.concat(late, ", ")
    end
  end,
}, "promote, remove, or reschedule the overdue backlog items (`prova specs --backlog`)")

--- Duration drift — "the run is now taking too long" as attention, never a hard-coded limit
--- (docs/design/reminders.md#duration-drift-is-attention). Every run records `run.duration_ms`
--- into the `timings` measurement set; once a machine deliberately banks it
--- (`--update-baseline=run.duration_ms`), this watcher trips when a run grows past 1.5× that
--- banked normal. Unbanked (this repo's committed default — durations are machine facts, not
--- repo facts), it WATCHES quietly. Passive either way: a context that means it as a gate says
--- `--heed=timings`.
prova.remind("run-duration-drift", {
  tags = { "timings" },
  when = function(a)
    local banked = a.baselines["run.duration_ms"]
    if not banked then
      return false
    end
    if a.duration_ms > banked * 1.5 then
      return string.format("this run took %.0fms against a banked %.0fms (over 1.5x)",
        a.duration_ms, banked)
    end
    return false
  end,
}, "the run outgrew its banked duration — a heavy proof in the wrong lane? Re-bank deliberately with --update-baseline=run.duration_ms if this is the new normal")
