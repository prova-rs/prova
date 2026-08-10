//! The `measure` namespace — the measurements seam (docs/design/verifiers.md).
//!
//! Where `junit` ingests another tool's *verdicts*, `measure` ingests its *scalars*: a file's line
//! count, a coverage percentage, a lint tally. A scalar is not a verdict — it becomes one only when
//! a ratchet compares it against the committed baseline. So the shape mirrors junit: `measure.record`
//! files a named value into the run's measurement account (a no-op with no registry, so an `eval`
//! pollutes nothing), and `measure.ratchet` (a Lua recipe over it) records AND asserts no regression
//! against `.prova/baselines/<set>.json`.
//!
//! The baseline is read here, in Lua, from `prova.root` (fs + json — both already installed). Writing
//! it is deliberately NOT here: only `prova --update-baseline` moves a baseline, and only ever
//! tighter unless the committed file is hand-edited. That asymmetry — read anywhere, write through
//! one guarded path — is the anti-gaming property, so it lives in the CLI, not in a recipe an agent
//! could rewrite.

use mlua::{Lua, Table};

use crate::model::{Direction, Measurement, MeasurementRegistry};

/// Map the Lua `direction` opt to the core enum. Anything but an explicit "higher" spelling is
/// lower-is-better — the common case (sizes, counts, ratios) and the safe default.
fn parse_direction(s: Option<String>) -> Direction {
    match s.as_deref() {
        Some("higher_is_better") | Some("higher") | Some("more") => Direction::HigherIsBetter,
        _ => Direction::LowerIsBetter,
    }
}

/// The `measure.ratchet` facet — Lua over `measure.record` plus a baseline read, exactly the shape
/// the design doc names. Kept as a recipe so the contract reads at `prova learn` level: record the
/// value, load the committed baseline, and fail loudly on a regression (or on a missing baseline —
/// a metric with no floor passes nothing until `--update-baseline` establishes it).
const RATCHET_RECIPE: &str = r##"
function measure.ratchet(t, name, value, opts)
  opts = opts or {}
  local dir = opts.direction or "lower_is_better"
  local set = opts.set or "default"
  -- File it into the run's account: this is what the record keeps and what `--update-baseline` reads.
  measure.record(name, value, { direction = dir, set = set })

  local path = prova.root .. "/.prova/baselines/" .. set .. ".json"
  local m = nil
  if fs.exists(path) then
    local data = json.decode(fs.read(path))
    m = data and data.metrics and data.metrics[name]
  end

  if m == nil then
    -- No committed baseline for this metric. Refuse to pass vacuously (mirrors matches_snapshot on a
    -- missing snapshot): establish it with `prova --update-baseline`, then this becomes a ratchet.
    t:expect(false, "measure.ratchet: no baseline for '" .. name .. "' in set '" .. set ..
      "' — run `prova --update-baseline` to establish it"):is_true()
    return value
  end

  local floor = m.value
  -- 1) The ceiling: never regress past the committed baseline (the preventive ratchet).
  if dir == "higher_is_better" then
    t:expect(value, name .. " regressed to " .. value .. " (baseline floor " .. floor ..
      ", higher is better) — recover it, or hand-edit the committed baseline if the regression is intended (--update-baseline refuses to loosen)")
      :gte(floor)
  else
    t:expect(value, name .. " regressed to " .. value .. " (baseline ceiling " .. floor ..
      ", lower is better) — bring it back down, or hand-edit the committed baseline if the regression is intended (--update-baseline refuses to loosen)")
      :never():gt(floor)
  end

  -- 2) Paydown: when the baseline declares a `goal`, drive toward it (docs/design/verifiers.md).
  -- The ceiling above still holds; this adds the proactive half — a goal met graduates (prova's
  -- idiom), and a `deadline` past turns the standing debt red.
  if m.goal ~= nil then
    local met
    if dir == "higher_is_better" then met = value >= m.goal else met = value <= m.goal end
    if met then
      -- Graduate on success, like a promise that starts passing: demand the gain be locked in and
      -- the goal retired, rather than letting a reached goal linger green forever.
      t:expect(false, name .. " reached its paydown goal " .. m.goal .. " (now " .. value ..
        ") — run `prova --update-baseline` to lock in the gain, then retire or lower the goal")
        :is_true()
    elseif m.deadline ~= nil then
      -- Past the deadline with the goal unmet is a hard failure: the debt came due.
      local y, mo, d = tostring(m.deadline):match("^(%d+)-(%d+)-(%d+)")
      if y then
        local due = os.time({ year = tonumber(y), month = tonumber(mo), day = tonumber(d),
          hour = 23, min = 59, sec = 59 })
        if os.time() > due then
          t:expect(false, name .. " missed its paydown deadline " .. m.deadline .. " — still " ..
            math.abs(value - m.goal) .. " from goal " .. m.goal .. " (now " .. value .. ")")
            :is_true()
        end
      end
    end
    -- Otherwise: still paying down, within time. The ceiling assertion stands; the remaining gap
    -- (value vs goal) is the worklist item a `prova owed` / reminder surfaces.
  end
  return value
end
"##;

pub(crate) fn make(lua: &Lua, measurements: Option<MeasurementRegistry>) -> mlua::Result<Table> {
    let measure = lua.create_table()?;

    // record(name, value, opts) — file a named scalar into the run's measurement account. opts:
    // `direction` ("lower_is_better" default | "higher_is_better"), `set` (baseline file, "default").
    // A no-op when no registry is attached (an `eval`, a bare embedder), so recording never leaks.
    measure.set(
        "record",
        lua.create_function(
            move |_, (name, value, opts): (String, f64, Option<Table>)| {
                let (direction, set) = match &opts {
                    Some(o) => (
                        parse_direction(o.get("direction")?),
                        o.get::<Option<String>>("set")?
                            .unwrap_or_else(|| "default".to_string()),
                    ),
                    None => (Direction::LowerIsBetter, "default".to_string()),
                };
                if let Some(registry) = measurements.as_ref() {
                    // Recover a poisoned lock: the account is a plain Vec, valid at every step.
                    registry
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(Measurement {
                            name,
                            value,
                            direction,
                            set,
                        });
                }
                Ok(value)
            },
        )?,
    )?;

    Ok(measure)
}

/// Load the `measure.ratchet` recipe — after `make`'s table is installed as the global.
pub(crate) fn install_recipe(lua: &Lua) -> mlua::Result<()> {
    lua.load(RATCHET_RECIPE).set_name("@prova/measure").exec()
}
