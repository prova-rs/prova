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
