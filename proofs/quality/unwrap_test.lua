-- Quality gate: production code does not sprout new .unwrap()/.expect() calls — both are latent
-- panics. Test code uses them freely (idiomatic), so we count only lib+bin targets via clippy's
-- restriction lints, which exclude tests, and ratchet the counts against the committed baseline in
-- .prova/baselines/quality.json (lower is better). No new ones allowed; removing them is welcome —
-- run `prova --profile quality --update-baseline` to tighten the floor once you have.
--
-- HEAVY (recompiles with the restriction lints enabled): behind the `quality` switch, same as
-- the clippy gate. One clippy invocation feeds both counts via a file-scoped fixture.

local restrict = prova.fixture("clippy_restrict", Scope.File, function()
  local r = shell.run(
    { "cargo", "clippy", "--workspace", "--lib", "--bins", "--all-features", "--",
      "-W", "clippy::unwrap_used", "-W", "clippy::expect_used" },
    { cwd = prova.root, merge_stderr = true }
  )
  return r.stdout or ""
end)

-- The count is clippy's own diagnostic tally for the lint (a stable, monotonic proxy for the site
-- count — more calls, higher number), which is exactly what a no-regression ratchet needs.
local function count(out, needle)
  local _, n = out:gsub(needle, "")
  return n
end

prova.test("production .unwrap() count does not regress past the baseline", { switch = "quality" }, function(t)
  measure.ratchet(t, "rust.unwrap.production", count(t:use(restrict), "used `unwrap%(%)`"), { set = "quality" })
end)

prova.test("production .expect() count does not regress past the baseline", { switch = "quality" }, function(t)
  measure.ratchet(t, "rust.expect.production", count(t:use(restrict), "used `expect%(%)`"), { set = "quality" })
end)
