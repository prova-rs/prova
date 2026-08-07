//! The `quality` namespace — the golden-path composition for code-quality gates (docs/design/
//! verifiers.md).
//!
//! `quality.gate{…}` is not a new mechanism; it composes the primitives (`measure.check`/`record`,
//! `prova.test`, `prova.remind`) into the common shape, and the surface it authors *is* the
//! observe↔enforce dial:
//!   - **enforce** → a **proof** (`prova.test` + `t:expect`) — the check gates the run (fatal).
//!   - **observe** → a **reminder** (`prova.remind` + `when`) — the same check surfaces, non-fatal.
//!
//! Posture defaults to `prova.quality.posture` (the resolved `[quality]` dial), overridable per call
//! with `enforce=`. The check is `measure.check` (committed baseline + paydown) or a fixed `limit=`.
//!
//! Language-agnostic: `quality.gate` knows nothing about Rust — a pack computes the scalar/finding
//! however its tooling allows (clippy, tokei, or bare `wc -l`) and hands it in as `value`. This is
//! pure Lua over the bundled primitives, so it is readable at `prova learn` level and forkable; a
//! pack that needs something bespoke drops to the primitives directly.

use mlua::{Lua, Table};

/// The `quality.gate` recipe — Lua over `measure`/`prova.test`/`prova.remind`. Authored as a recipe
/// (like `junit.verify`) so the golden path is transparent and forkable.
const GATE_RECIPE: &str = r##"
function quality.gate(spec)
  local name = spec.name or error("quality.gate: name is required")
  local metric = spec.metric or name
  local value = spec.value
  if value == nil then error("quality.gate: value is required for '" .. name .. "'") end
  local set = spec.set or "default"
  local dir = spec.direction or "lower_is_better"

  -- Posture: an explicit enforce= wins; otherwise the project default from prova.quality.
  local enforce
  if spec.enforce ~= nil then
    enforce = spec.enforce
  else
    enforce = ((prova.quality and prova.quality.posture) or "enforce") == "enforce"
  end

  -- Record once, here at authoring time, so the run record and `--update-baseline` see the value
  -- regardless of which surface is authored below.
  measure.record(metric, value, { direction = dir, set = set })

  -- The check: a fixed threshold (`limit=`) or the committed baseline (via measure.check). Returns a
  -- why-string on violation, else nil. `value` is captured, so the reminder form needs no run state.
  local function evaluate()
    if spec.limit ~= nil then
      if dir == "higher_is_better" then
        if value < spec.limit then
          return name .. " is " .. value .. " (below floor " .. spec.limit .. ")"
        end
      elseif value > spec.limit then
        return name .. " is " .. value .. " (over limit " .. spec.limit .. ")"
      end
      return nil
    end
    return measure.check(metric, value,
      { direction = dir, set = set, goal = spec.goal, deadline = spec.deadline })
  end

  if enforce then
    -- Proof: the violation gates the run.
    prova.test(name, { requires = spec.requires }, function(t)
      local why = evaluate()
      t:expect(why == nil, why or (name .. ": within limit")):is_true()
    end)
  else
    -- Reminder: the same check surfaces (DUE) but never fails the run (unless heeded).
    prova.remind(name, { when = function(_) return evaluate() end, requires = spec.requires },
      spec.message or (name .. " — quality gate (observe); see the reported reason"))
  end
end
"##;

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    // No native functions — the `quality` namespace is pure composition. The empty table is the
    // global the recipe hangs `gate` on (installed after all modules exist).
    lua.create_table()
}

/// Load the `quality.gate` recipe — after `make`'s table is installed as the global and the
/// `measure`/`prova` globals it composes exist.
pub(crate) fn install_recipe(lua: &Lua) -> mlua::Result<()> {
    lua.load(GATE_RECIPE).set_name("@prova/quality").exec()
}
