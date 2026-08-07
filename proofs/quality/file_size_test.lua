-- Quality gate (file size), re-homed onto quality.gate so it honors the project's [quality] posture
-- (enforce => a proof that gates; observe => a reminder that only surfaces) and reads its limit from
-- prova.quality.max_file_lines. A big file is where bugs hide and where an agent loses the thread.
--
-- Per-file ceiling with the current giants grandfathered (recorded as known debt, with a graduation
-- check so a giant that drops under the limit demands removal from the list). The total-lines ratchet
-- (which defeats "split the mess in two to game per-file") rides the committed baseline and is added
-- in the dogfood phase.
--
-- Language-agnostic in spirit: line-count is the one metric every language's tooling can produce, so
-- this is the gate that works even where the whole toolbox is `wc -l`. The Rust specifics are only
-- the source roots below.

local LIMIT = (prova.quality and prova.quality.max_file_lines) or 1500

local SRC_ROOTS = {
  "crates/prova-core/src",
  "crates/prova-cli/src",
  "crates/prova-archetect/src",
  "xtask/src",
}

-- Known giants, repo-relative. Grandfathered, not excused — the graduation gate below turns any that
-- drop under the limit red, demanding they be removed from this list (the paydown win).
local GRANDFATHERED = {
  ["crates/prova-core/src/modules.rs"] = true,
  ["crates/prova-core/src/engine.rs"] = true,
  ["crates/prova-cli/src/main.rs"] = true,
}

local function line_count(path)
  local _, n = fs.read(path):gsub("\n", "")
  return n
end

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

local files = source_files()
local over, graduated = {}, {}
for _, f in ipairs(files) do
  local n = line_count(f.path)
  if GRANDFATHERED[f.rel] then
    if n <= LIMIT then
      graduated[#graduated + 1] = f.rel .. " (" .. n .. ")"
    end
  elseif n > LIMIT then
    over[#over + 1] = f.rel .. " (" .. n .. ")"
  end
end

-- Vacuity guard: a broken glob would make the gates below trivially pass. Always enforced.
quality.gate {
  name = "quality:file-size scanned the source tree",
  metric = "rust.file.scanned",
  value = #files,
  limit = 20,
  direction = "higher_is_better",
  enforce = true,
  message = "no source files scanned — SRC_ROOTS or glob is wrong",
}

-- The per-file ceiling (honors posture).
quality.gate {
  name = "no source file exceeds " .. LIMIT .. " lines",
  metric = "rust.file.oversized",
  value = #over,
  limit = 0,
  message = "oversized files (split them, or grandfather with a paydown note): " .. table.concat(over, ", "),
}

-- Graduation: a grandfathered giant now under the limit must be removed from GRANDFATHERED (paid down!).
quality.gate {
  name = "grandfathered giants that have been paid down are retired",
  metric = "rust.file.graduated",
  value = #graduated,
  limit = 0,
  message = "now <= " .. LIMIT .. " — remove from GRANDFATHERED (paid down!): " .. table.concat(graduated, ", "),
}
