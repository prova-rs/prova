--- Draw-down: a date on an anchor, plus a reminder, makes a parked item accountable to a clock.
---
--- This is the payoff of dated anchors. A reminder's `when` receives `account.dated` — every
--- claim/backlog anchor carrying a `YYYY-MM-DD` — and `date.past(o.date)` asks whether a deadline
--- has passed. Before the date the reminder is silent (WATCHING); after it, DUE; under `--heed`,
--- fatal. The item did not become owed — a human still promotes it — but it stopped being able to
--- rot unnoticed. One `when` over `account.dated` draws down authored dates (anchors) today and
--- computed ones (deprecations) later — the same surface.

local function drawdown_project(t, date_str)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.mkdir(proj .. "/docs")
  fs.write(proj .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n')
  fs.write(proj .. "/docs/design.md",
    "<!-- backlog: shape-me " .. date_str .. " -->\nA backlog item with a draw-down deadline.\n")
  fs.write(proj .. "/proofs/drawdown_test.lua", [[
prova.remind("backlog-drawdown", {
  when = function(a)
    local late = {}
    for _, o in ipairs(a.dated) do
      if o.kind == "backlog" and date.past(o.date) then late[#late + 1] = o.address end
    end
    if #late > 0 then
      return #late .. " backlog item(s) past their draw-down date"
    end
  end,
}, "promote or drop the overdue backlog items")

prova.test("a real proof so the run is not empty", function(t) t:expect(1):equals(1) end)
]])
  return proj
end

prova.test("a backlog item past its draw-down date fires the reminder DUE", {
  proves = "the payoff of a date on an anchor: a reminder reads `account.dated`, compares each deadline to now, and goes DUE once one passes — a parked item becomes accountable to a clock without being forced owed",
}, function(t)
  local proj = drawdown_project(t, "2000-01-01") -- long past
  shell.run(prova.bin, { cwd = proj, merge_stderr = true }) -- a run evaluates + records reminders
  local r = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })

  t:expect(r.stdout, "the overdue item is due"):contains("DUE")
  t:expect(r.stdout, "the reminder names itself"):contains("backlog-drawdown")
  t:expect(r.code, "a due reminder exits non-zero"):never():equals(0)
end)

prova.test("a backlog item with a future date stays WATCHING", {
  proves = "the same condition is silent while there is still time — a draw-down date is a deadline, not an alarm the moment it is set",
}, function(t)
  local proj = drawdown_project(t, "2999-01-01") -- far future
  shell.run(prova.bin, { cwd = proj, merge_stderr = true })
  local r = shell.run(prova.bin .. " reminders", { cwd = proj, merge_stderr = true })

  t:expect(r.stdout, "not yet due"):never():contains("DUE")
  t:expect(r.code, "nothing due, so it exits clean"):equals(0)
end)
