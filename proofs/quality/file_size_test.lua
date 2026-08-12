-- Quality gate: no Rust source file grows without bound. A big file is where bugs hide and where
-- an agent loses the thread, so oversized files are a red condition that forces a refactor.
--
-- Posture: the COUNT of files past the limit is ratcheted, and the count is ZERO — the giants
-- this gate once grandfathered (modules.rs 7,443 · engine.rs 6,654 · main.rs 5,203) were all
-- paid down in 2026-08, so at zero the ratchet IS the hard limit: any new giant is a 0 → 1
-- regression, red immediately, and every offender is named in the output. (The grandfather-list
-- machinery this replaces earned its keep while the giants existed; an empty list was just
-- ceremony around the same enforcement.)
--
-- Layout-agnostic: the roots scanned come from `cargo metadata` (workspace.src_roots), never a
-- hardcoded list — a hardcoded list goes silently stale the day a crate is added.

local workspace = require("workspace")

local LIMIT = 1500

-- wc -l semantics: count newlines, so the numbers match a plain `wc -l` and each other.
local function line_count(path)
  local _, n = fs.read(path):gsub("\n", "")
  return n
end

-- fs.glob's base is a concrete dir; "*.rs" catches files directly under it and "**/*.rs" the nested
-- ones. Globbing both and de-duping is robust regardless of whether "**" also matches depth zero.
local function source_files(roots)
  local seen, out = {}, {}
  for _, root in ipairs(roots) do
    for _, pat in ipairs({ "*.rs", "**/*.rs" }) do
      for _, path in ipairs(fs.glob(root, pat)) do
        if not seen[path] then
          seen[path] = true
          out[#out + 1] = path
        end
      end
    end
  end
  return out
end

prova.test("oversized source files (> " .. LIMIT .. " lines) do not multiply past the baseline", {
  locks = { prova.reads("cargo") },
  switch = "quality",
  requires = { "cargo" },
}, function(t)
  local files = source_files(workspace.src_roots(t:use(workspace.metadata)))
  -- Vacuity guard: a broken metadata/glob answer would make the count below trivially zero.
  t:expect(#files, "suspiciously few source files scanned — src-root discovery is wrong"):gt(20)

  local prefix = prova.root .. "/"
  local over = 0
  for _, path in ipairs(files) do
    local n = line_count(path)
    if n > LIMIT then
      over = over + 1
      local rel = path:sub(1, #prefix) == prefix and path:sub(#prefix + 1) or path
      print(string.format("  oversized  %-60s %6d lines", rel, n))
    end
  end
  measure.ratchet(t, "rust.files.oversized", over, { set = "quality" })
end)
