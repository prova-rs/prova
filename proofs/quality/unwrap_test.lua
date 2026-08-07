-- Quality pack (Rust): production .unwrap()/.expect() do not grow. Re-homed onto quality.gate so it
-- honors the [quality] posture (enforce => proof, observe => reminder). HEAVY (shells clippy) => it
-- requires the `quality` capability, so a plain `prova` run skips it.
--
-- ANTI-GAMING: the metric counts EVERY production site, including ones a `#[allow(clippy::unwrap_used)]`
-- would normally silence — because we run the restriction lints with `--force-warn`, which overrides
-- allow/expect attributes. So you cannot pay the gate down by hiding a call behind `#[allow]`; the
-- only way the number falls is to remove the call. (`#[allow]` remains the honest tool for a genuinely
-- necessary unwrap — it just doesn't fool the census.)
--
-- clippy's restriction lints run on --lib/--bins, which exclude test code (unwraps in tests are fine).

-- One clippy run, memoized across both gates (same worker Lua state). --force-warn (not -W) so
-- suppressed sites are still counted.
local _out
local function clippy_out()
  if _out == nil then
    local r = shell.run(
      { "cargo", "clippy", "--workspace", "--lib", "--bins", "--all-features", "--",
        "--force-warn", "clippy::unwrap_used", "--force-warn", "clippy::expect_used" },
      { cwd = prova.root, merge_stderr = true })
    _out = r.stdout or ""
  end
  return _out
end

local function sites(needle)
  local _, n = clippy_out():gsub(needle, "")
  return n
end

quality.gate {
  name = "production .unwrap() count stays within the baseline",
  metric = "rust.unwrap.production",
  set = "quality",
  requires = { "quality" },
  probe = function() return sites("used `unwrap%(%)`") end,
}

quality.gate {
  name = "production .expect() count stays within the baseline",
  metric = "rust.expect.production",
  set = "quality",
  requires = { "quality" },
  probe = function() return sites("used `expect%(%)`") end,
}
