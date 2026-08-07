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
-- measure.check(name, value, opts) -> why (string) | nil
-- The ratchet comparison as a PURE function: no assert, no record. Returns a human why-string when
-- the value violates the committed baseline (regression, or paydown goal-met/deadline-past), else
-- nil. The single source of truth shared by measure.ratchet (proof) and quality.gate (either
-- surface) — so the guard logic never drifts between them.
function measure.check(name, value, opts)
  opts = opts or {}
  local dir = opts.direction or "lower_is_better"
  local set = opts.set or "default"
  local goal = opts.goal
  local deadline = opts.deadline

  local path = prova.root .. "/.prova/baselines/" .. set .. ".json"
  local m = nil
  if fs.exists(path) then
    local data = json.decode(fs.read(path))
    m = data and data.metrics and data.metrics[name]
  end
  -- opts.goal/deadline (from a caller like quality.gate) win over the file's, so a gate can carry a
  -- paydown target even before it is committed to the baseline.
  if m ~= nil then
    if goal == nil then goal = m.goal end
    if deadline == nil then deadline = m.deadline end
  end

  if m == nil then
    -- No committed baseline. Refuse to pass vacuously (mirrors matches_snapshot on a missing
    -- snapshot): establish it with `prova --update-baseline`, then this becomes a ratchet.
    return "no baseline for '" .. name .. "' in set '" .. set ..
      "' — run `prova --update-baseline` to establish it"
  end

  local floor = m.value
  -- The ceiling: never regress past the committed baseline (the preventive ratchet).
  if dir == "higher_is_better" then
    if value < floor then
      return name .. " regressed to " .. value .. " (baseline floor " .. floor ..
        ", higher is better) — recover it, or lower the baseline via --update-baseline if intended"
    end
  else
    if value > floor then
      return name .. " regressed to " .. value .. " (baseline ceiling " .. floor ..
        ", lower is better) — bring it back down, or raise the baseline via --update-baseline if intended"
    end
  end

  -- Paydown: when a `goal` is declared, drive toward it. Goal met graduates (prova's idiom); a
  -- `deadline` past with the goal unmet turns the standing debt red.
  if goal ~= nil then
    local met
    if dir == "higher_is_better" then met = value >= goal else met = value <= goal end
    if met then
      return name .. " reached its paydown goal " .. goal .. " (now " .. value ..
        ") — run `prova --update-baseline` to lock in the gain, then retire or lower the goal"
    elseif deadline ~= nil then
      local y, mo, d = tostring(deadline):match("^(%d+)-(%d+)-(%d+)")
      if y then
        local due = os.time({ year = tonumber(y), month = tonumber(mo), day = tonumber(d),
          hour = 23, min = 59, sec = 59 })
        if os.time() > due then
          return name .. " missed its paydown deadline " .. deadline .. " — still " ..
            math.abs(value - goal) .. " from goal " .. goal .. " (now " .. value .. ")"
        end
      end
    end
  end
  return nil
end

-- measure.ratchet(t, name, value, opts) — the proof form: record the value, then assert the check.
function measure.ratchet(t, name, value, opts)
  opts = opts or {}
  measure.record(name, value, { direction = opts.direction or "lower_is_better", set = opts.set or "default" })
  local why = measure.check(name, value, opts)
  t:expect(why == nil, why or (name .. ": within baseline")):is_true()
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
                    registry
                        .lock()
                        .expect("measurement registry")
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
