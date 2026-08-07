-- Spec for the `date` convenience (docs/design/reminders.md): thin ergonomic helpers over os.*, so a
-- reminder's `when` expresses schedules/deadlines without hand-rolling os.time math. Time is a
-- qualifier a condition composes, not a mechanism. January dates throughout — no DST seam.

prova.test("date.parse/format round-trip a calendar date", function(t)
  t:expect(date.format(date.parse("2026-01-07"))):equals("2026-01-07")
end)

prova.test("date.parse reads an optional time; format renders it", function(t)
  t:expect(date.format(date.parse("2026-01-07 06:30:00"), "%H:%M")):equals("06:30")
end)

prova.test("date.diff_days counts whole days between dates, either direction", function(t)
  t:expect(date.diff_days("2026-01-01", "2026-01-31")):equals(30)
  t:expect(date.diff_days("2026-01-31", "2026-01-01")):equals(-30)
end)

prova.test("date.past / days_since / days_until answer relative to now", function(t)
  t:expect(date.past("2000-01-01")):is_true()
  t:expect(date.past("2999-12-31")):equals(false)
  t:expect(date.days_since("2000-01-01")):gt(0)
  t:expect(date.days_until("2999-12-31")):gt(0)
  t:expect(date.now()):gt(0)
end)

prova.test("helpers accept a timestamp number as well as a date string", function(t)
  local ts = date.parse("2026-01-01")
  t:expect(date.diff_days(ts, "2026-01-11")):equals(10)
end)

prova.test("date.parse rejects malformed input", function(t)
  local ok = pcall(function() date.parse("not-a-date") end)
  t:expect(ok):equals(false)
end)

-- The point of it all: a scheduled reminder condition reads cleanly (this is the whole use case).
prova.test("reads naturally in a reminder-style condition", function(t)
  local overdue = date.past("2000-01-01") and date.days_since("2000-01-01") > 30
  t:expect(overdue):is_true()
end)
