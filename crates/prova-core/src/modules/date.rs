//! The `date` namespace — ergonomic dates/times for reminder conditions (and anywhere).
//!
//! A thin CONVENIENCE over Lua's `os.time`/`os.date`, not a scheduling capability. It exists only so
//! a reminder's `when` can read `date.days_since("2026-01-01") > 30` or `date.past(deadline)` instead
//! of hand-rolling `os.time({...})` math. Time is a *qualifier* a condition composes — never a
//! property baked into measurements, baselines, or ratchets (docs/design/reminders.md).
//!
//! Pure Lua (no native code, no new deps): the calendar math is `os.*`, which prova's Lua already
//! exposes. Strings are `"YYYY-MM-DD"` with an optional `"[T ]HH:MM:SS"`; instants are unix seconds,
//! interpreted in local time (consistent for the date comparisons this is for).

use mlua::{Lua, Table};

const DATE_RECIPE: &str = r##"
do
  local function parse(s)
    local y, mo, d, h, mi, se =
      tostring(s):match("^(%d%d%d%d)%-(%d%d)%-(%d%d)[T ]?(%d?%d?):?(%d?%d?):?(%d?%d?)")
    if not y then
      error("date.parse: expected \"YYYY-MM-DD\" (optionally \" HH:MM:SS\"), got " .. tostring(s))
    end
    return os.time({
      year = tonumber(y), month = tonumber(mo), day = tonumber(d),
      hour = tonumber(h) or 0, min = tonumber(mi) or 0, sec = tonumber(se) or 0,
    })
  end
  -- Accept a unix-seconds number as-is, or parse a date string — so callers write either.
  local function to_ts(v)
    if type(v) == "number" then return v end
    if type(v) == "string" then return parse(v) end
    error("date: expected a timestamp number or a \"YYYY-MM-DD\" string, got " .. type(v))
  end
  local DAY = 86400

  date.now = function() return os.time() end
  date.parse = parse
  date.format = function(ts, fmt) return os.date(fmt or "%Y-%m-%d", ts or os.time()) end
  -- whole days from a to b (b - a), floored; args may be timestamps or date strings.
  date.diff_days = function(a, b) return math.floor((to_ts(b) - to_ts(a)) / DAY) end
  date.days_since = function(v) return math.floor((os.time() - to_ts(v)) / DAY) end
  date.days_until = function(v) return math.floor((to_ts(v) - os.time()) / DAY) end
  date.past = function(v) return os.time() > to_ts(v) end
end
"##;

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    // No native functions — the recipe below hangs the calendar helpers on this table (installed
    // after all modules exist, like the other recipes).
    lua.create_table()
}

/// Load the `date.*` recipe — after `make`'s table is installed as the global.
pub(crate) fn install_recipe(lua: &Lua) -> mlua::Result<()> {
    lua.load(DATE_RECIPE).set_name("@prova/date").exec()
}
