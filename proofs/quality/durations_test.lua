-- Quality gate: no boundary in the tree drops a duration it cannot read
-- (agent-ergonomics.md#unparseable-durations-are-dropped-not-refused). The BEHAVIOR proofs — that
-- each site refuses, and that the real grammar is still accepted — are fast and flag-free in
-- proofs/spec/durations/. This is the structural half: it scans the source, so it takes the cargo
-- lock and lives behind the quality switch with its siblings.

local workspace = require("workspace")

--- The sweep itself, held structurally — because the backlog item's own advice was to audit this
--- as ONE sweep rather than one call site: `and_then(parse_duration)` is the idiom wherever a
--- duration crosses from Lua, and every instance of it fails the same way. A table of examples
--- proves the sites it lists; this proves there are no others, including ones added tomorrow.
prova.test("no boundary still drops a duration it cannot read", {
  covers = "docs/design/agent-ergonomics.md#unparseable-durations-are-dropped-not-refused",
  -- BEHIND THE SWITCH, like every sibling in this directory, and for a load-bearing reason: this
  -- proof takes the cargo lock, and the coverage conduct HOLDS that lock while it runs the
  -- black-box suite. In the default lane the inner suite blocked on a lock its own conductor
  -- owned and sat there until the 1200s timeout — a deadlock that presents as "coverage is slow".
  -- A proof that reaches for cargo belongs in the lane that already prices cargo.
  switch = "quality",
  requires = { "cargo" },
  locks = { prova.reads("cargo") },
  proves = "durations: the sweep is complete and stays complete — the dropping idiom is gone from the tree",
}, function(t)
  local roots = workspace.src_roots(t:use(workspace.metadata))
  t:expect(#roots, "no source roots to scan — cargo metadata found nothing"):gt(0)

  local offenders = {}
  local scanned = 0
  for _, root in ipairs(roots) do
    for _, pat in ipairs({ "*.rs", "**/*.rs" }) do
      for _, path in ipairs(fs.glob(root, pat)) do
        scanned = scanned + 1
        local src = fs.read(path)
        -- The dropping shape: a fallible parse fed straight into `and_then`, which turns "I could
        -- not read that" into "you did not ask for one".
        -- `%(` because a bare `(` opens a CAPTURE in a Lua pattern: the unescaped form matched
        -- nothing at all, and this proof was vacuously green until a mutation test said so.
        if src:find("and_then%(%s*|s|%s*[%w_:]*parse_duration") then
          offenders[#offenders + 1] = path
        end
      end
    end
  end

  -- Vacuity guard: a broken glob would make the count trivially zero and this proof a no-op.
  t:expect(scanned, "suspiciously few files scanned — src-root discovery is wrong"):gt(20)
  t:expect(#offenders, "these still drop a malformed duration: " .. table.concat(offenders, ", "))
    :equals(0)
end)
