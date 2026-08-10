-- Quality gate: no Rust source file grows without bound. A big file is where bugs hide and where
-- an agent loses the thread, so oversized files are a red condition that forces a refactor.
--
-- Posture (Mix): a HARD limit for new files; the current giants are GRANDFATHERED — recorded here
-- as known debt so CI stays green today, but each one carries a graduation check (drops to <=
-- LIMIT -> FAILS demanding delisting) AND a per-file line-count ratchet: growth past the committed
-- baseline is red, every shrink is banked by --update-baseline, and a `goal` in the baseline file
-- schedules the paydown. A god file taxes users and agents alike — the ratchet is the pressure,
-- modularization sessions are the payments.

local LIMIT = 1500

-- Source trees prova gates on its own code (the four crate/tool src roots; never target/ or tests).
local SRC_ROOTS = {
  "crates/prova-core/src",
  "crates/prova-cli/src",
  "crates/prova-archetect/src",
  "xtask/src",
}

-- Known giants, repo-relative. Grandfathered, not excused: still red-in-waiting via the graduation
-- check below. Splitting any of these is the first paydown target.
local GRANDFATHERED = {
  ["crates/prova-core/src/modules.rs"] = true,
  ["crates/prova-core/src/engine.rs"] = true,
  ["crates/prova-cli/src/main.rs"] = true,
}

-- wc -l semantics: count newlines, so the numbers match a plain `wc -l` and each other.
local function line_count(path)
  local _, n = fs.read(path):gsub("\n", "")
  return n
end

-- fs.glob's base is a concrete dir; "*.rs" catches files directly under it and "**/*.rs" the nested
-- ones. Globbing both and de-duping is robust regardless of whether "**" also matches depth zero.
local function source_files()
  local prefix = prova.root .. "/"
  local seen, out = {}, {}
  for _, root in ipairs(SRC_ROOTS) do
    for _, pat in ipairs({ "*.rs", "**/*.rs" }) do
      for _, path in ipairs(fs.glob(prefix .. root, pat)) do
        if not seen[path] then
          seen[path] = true
          local rel = path:sub(1, #prefix) == prefix and path:sub(#prefix + 1) or path
          out[#out + 1] = { path = path, rel = rel }
        end
      end
    end
  end
  return out
end

prova.test("no source file exceeds " .. LIMIT .. " lines (giants grandfathered, tracked for paydown)", function(t)
  local files = source_files()
  -- Vacuity guard: a broken glob/root would make every assertion below trivially pass.
  t:expect(#files, "no source files scanned — SRC_ROOTS or glob is wrong"):gt(20)

  for _, f in ipairs(files) do
    local n = line_count(f.path)
    if GRANDFATHERED[f.rel] then
      -- Still legitimately a giant. When it finally drops to <= LIMIT this fails, demanding you
      -- remove it from GRANDFATHERED — the graduation / paydown signal.
      t:expect(n, f.rel .. " is now <= " .. LIMIT .. " lines — remove it from GRANDFATHERED (paid down!)"):gt(LIMIT)
    else
      t:expect(n, f.rel .. " is " .. n .. " lines (> " .. LIMIT .. ") — split it, or grandfather it with a paydown note"):never():gt(LIMIT)
    end
  end
end)

prova.test("the grandfathered giants do not grow — each file's line count is ratcheted", { switch = "quality" }, function(t)
  -- The debt, numerically: growth past a giant's committed baseline is red; a shrink is banked
  -- with --update-baseline; a `goal` in the baseline file schedules the modularization.
  for rel in pairs(GRANDFATHERED) do
    local metric = "rust.file_lines." .. rel:match("([^/]+)%.rs$")
    measure.ratchet(t, metric, line_count(prova.root .. "/" .. rel), { set = "quality" })
  end
end)
