--- prova's own quality gates, as reminders — the attention account prova holds ITSELF to.
---
--- Distinct from the proofs, which are the evidence account (what prova DOES). A reminder is a
--- standing condition the world can trip: it reports (WATCHING → DUE) and a context elevates it to
--- fatal with `--heed` (docs/design/reminders.md). This file is where prova dogfoods its own gates;
--- add one as it earns its place. Reminders here are evaluated on every full run and reported by
--- `prova reminders`.

--- Draw down prova's own dated backlog. A `<!-- backlog: id YYYY-MM-DD -->` past its deadline is
--- work prova said it would do and hasn't — most immediately, removing the deprecation bridges in
--- docs/design/deprecations.md. WATCHING while there is time; DUE, with the addresses named, once a
--- date passes. It never forces the work owed — a human still promotes or reschedules — but it
--- cannot rot unseen.
prova.remind("backlog-drawdown", {
  when = function(a)
    local late = {}
    for _, o in ipairs(a.dated) do
      if o.kind == "backlog" and date.past(o.date) then
        late[#late + 1] = o.address
      end
    end
    if #late > 0 then
      return #late .. " backlog item(s) past their draw-down date: " .. table.concat(late, ", ")
    end
  end,
}, "promote, remove, or reschedule the overdue backlog items (`prova backlog`)")
