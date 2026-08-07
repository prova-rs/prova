-- Quality pack (Rust): production .unwrap()/.expect() do not grow. Re-homed onto quality.gate so it
-- honors the [quality] posture (enforce => proof, observe => reminder). HEAVY (shells clippy) => it
-- requires the `quality` capability, so a plain `prova` run skips it.
--
-- ANTI-GAMING: the metric is live clippy diagnostics PLUS committed #[allow(clippy::unwrap_used)]
-- suppressions across the source. Silencing a site with #[allow] therefore does NOT shrink the
-- number — it only moves it from the "live" bucket to the "documented exception" bucket, and both are
-- counted. So you cannot pay down the gate by hiding; only by removing the call. (Requiring a reason
-- on each allow is a further hardening tracked separately — it needs a cleanup of existing allows.)
--
-- clippy's restriction lints run on --lib/--bins, which exclude test code (unwraps in tests are fine).

local SRC_ROOTS = "crates/prova-core/src crates/prova-cli/src crates/prova-archetect/src xtask/src"

-- One clippy run, memoized across both gates (same worker Lua state).
local _out
local function clippy_out()
  if _out == nil then
    local r = shell.run(
      { "cargo", "clippy", "--workspace", "--lib", "--bins", "--all-features", "--",
        "-W", "clippy::unwrap_used", "-W", "clippy::expect_used" },
      { cwd = prova.root, merge_stderr = true })
    _out = r.stdout or ""
  end
  return _out
end

local function diagnostics(needle)
  local _, n = clippy_out():gsub(needle, "")
  return n
end

-- Count committed suppressions of a restriction lint across the source roots (documented debt).
local function suppressions(lint)
  local r = shell.run({ "bash", "-c",
    "grep -rIoh 'allow(clippy::" .. lint .. "' " .. SRC_ROOTS .. " 2>/dev/null | wc -l" },
    { cwd = prova.root })
  return tonumber((r.stdout or ""):match("%d+") or "0") or 0
end

quality.gate {
  name = "production .unwrap() (live + suppressed) stays within the baseline",
  metric = "rust.unwrap.production",
  set = "quality",
  requires = { "quality" },
  probe = function() return diagnostics("used `unwrap%(%)`") + suppressions("unwrap_used") end,
}

quality.gate {
  name = "production .expect() (live + suppressed) stays within the baseline",
  metric = "rust.expect.production",
  set = "quality",
  requires = { "quality" },
  probe = function() return diagnostics("used `expect%(%)`") + suppressions("expect_used") end,
}
