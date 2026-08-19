--- **A duration prova cannot read is refused, never dropped.**
---
--- This is the one value where best-effort parsing produces exactly the failure the option exists
--- to prevent. `timeout = "30 seconds"` is a plausible spelling of a real grammar (`"30s"`), and
--- dropping it configures NO BOUND AT ALL — so the proof that meant to be bounded waits forever,
--- and the only symptom is a hung CI job hours later.
---
--- It is the closed-opts doctrine one level down. `opts::Closed` closed the KEY set, so `tiemout`
--- is refused by name; the value under a correctly-spelled key was still parsed best-effort, and
--- the two mistakes look identical to an author who typed one character wrong in either place.

-- The workspace recipe loads with the FILE: a fixture must be registered before the run's plan is
-- sealed, so a `require` inside a test body would arrive too late to be `t:use`d.
local workspace = require("workspace")

--- Every boundary that takes a duration string, and the shape of the mistake at each.
local sites = {
  { name = "the unit `timeout` — the bound a proof asks for BY NAME",
    code = 'prova.test("t", { timeout = "30 seconds" }, function(t) end)' },
  { name = "shell.run timeout",      code = 'shell.run("true", { timeout = "1 minute" })' },
  { name = "shell.run idle_timeout", code = 'shell.run("true", { idle_timeout = "ages" })' },
  { name = "shell.run first_byte",   code = 'shell.run("true", { first_byte = "soon" })' },
  { name = "http.client timeout",    code = 'http.client({ base_url = "http://127.0.0.1:1", timeout = "5 seconds" })' },
  { name = "http.wait_for every",    code = 'http.wait_for("http://127.0.0.1:1", { every = "often" })' },
  { name = "prova.retry timeout",    code = 'prova.retry(function() return true end, { timeout = "a while" })' },
  { name = "grpc.client timeout",    code = 'grpc.client("127.0.0.1:1", { timeout = "quick" })' },
}

prova.test_each("a malformed duration is refused: {name}", sites, function(t, case)
  local r = shell.run({ prova.bin, "eval", case.code }, { merge_stderr = true, timeout = "60s" })

  t:expect(r.code, case.name .. " must REFUSE, not drop"):never():equals(0)
  -- The message has to carry the grammar, because the author's next question is always "then
  -- what IS the spelling?" — and a refusal that does not answer it just moves the guessing.
  t:expect(r.stdout, "the refusal teaches the grammar"):contains("is not a duration")
  t:expect(r.stdout):contains("250ms")
end)

--- The negative control the table above needs: the accepted grammar still passes, at the same
--- sites, so those refusals are measuring a parser rather than a gate that refuses everything.
prova.test("every spelling of the real grammar is accepted", {
  proves = "durations: the refusal is a parser, not a wall — the negative control for the table above",
}, function(t)
  local ok = shell.run({ prova.bin, "eval", [[
    shell.run("true", { timeout = "1m", idle_timeout = "250ms", first_byte = "0s" })
    prova.retry(function() return true end, { timeout = "2s", every = "10ms" })
    return "fine"
  ]] }, { merge_stderr = true, timeout = "60s" })
  t:expect(ok.code, ok.stdout):equals(0)
  t:expect(ok.stdout):contains("fine")

  -- A bare number is seconds — the one form with no unit, and it must not read as malformed.
  local bare = shell.run({ prova.bin, "eval", 'shell.run("true", { timeout = "5" }) return "ok"' },
    { merge_stderr = true, timeout = "60s" })
  t:expect(bare.code, bare.stdout):equals(0)
end)

--- The sweep itself, held structurally — because the backlog item's own advice was to audit this
--- as ONE sweep rather than one call site: `and_then(parse_duration)` is the idiom wherever a
--- duration crosses from Lua, and every instance of it fails the same way. A table of examples
--- proves the sites it lists; this proves there are no others, including ones added tomorrow.
prova.test("no boundary still drops a duration it cannot read", {
  covers = "docs/design/agent-ergonomics.md#unparseable-durations-are-dropped-not-refused",
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
