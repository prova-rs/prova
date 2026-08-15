--- No gate may quietly leave the path: every switched class is thrown by some profile
--- (docs/design/manifest.md#switches-not-env-capabilities).
---
--- A `switch = "<class>"` is OFF unless thrown, which is what makes an expensive leg optional in
--- the inner loop. The hazard is the other end of that: a class nothing throws is not optional, it
--- is GONE — and it goes quietly, because a run that never selects it reports it as switched off,
--- which is the same line a correctly-optional class prints.
---
--- Measured, which is why this proof exists: `coverage` — a RATCHET, so a gate — was thrown only by
--- its own profile, sat outside the pre-push sweep, and its scheduled CI lane failed for three
--- consecutive days with a real regression (unit coverage 73.46 -> 72.26) that nobody was told
--- about. Nothing was broken; it had simply stopped being on anyone's path.
---
--- The exemption is ENUMERATED, the way `parity_test.lua` enumerates retired spellings: a class may
--- be ad-hoc only when it is an INSTRUMENT rather than a gate, and it has to say so here, by name,
--- with the reason. An accidental orphan must still fail.

--- Classes deliberately thrown by no profile, and why. Being on this list is a claim that the
--- class answers a question ABOUT THE HOST or the world, not about whether this tree is fit to
--- land — so no sweep should wait on it.
local INSTRUMENTS = {
  soak = "a 2x2 experiment over container runtimes: it characterizes whether Docker Desktop binds "
    .. "the ports it claims to expose. A red soak is a finding about the runtime, not a regression "
    .. "in this tree, and it runs for hours — nothing that gates a commit can wait on it",
}

--- Every class the binary reports, with who throws it — read from `prova switches` so the proof
--- sees exactly what an author would.
local function classes(t)
  local r = shell.run({ prova.bin, "switches" }, { merge_stderr = true, timeout = "60s" })
  t:expect(r.code, "`prova switches` answers: " .. r.stdout):equals(0)

  local out = {}
  for line in r.stdout:gmatch("[^\n]+") do
    local name, thrown = line:match("^%s+([%w%-_]+)%s+%d+ gated · thrown by: (.+)$")
    if name then
      out[#out + 1] = { name = name, thrown = thrown, orphan = thrown:find("nobody") ~= nil }
    end
  end
  return out
end

prova.test("every switched class is thrown by a profile, or named here as an instrument", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "an orphaned class reports identically to a correctly-optional one — both say `switched off` — so nothing in a run's output distinguishes 'you chose not to run this' from 'nobody can run this any more'. `coverage` spent three days in the second state while reading like the first",
}, function(t)
  local orphans = {}
  for _, c in ipairs(classes(t)) do
    if c.orphan and not INSTRUMENTS[c.name] then
      orphans[#orphans + 1] = c.name
    end
  end
  t:expect(table.concat(orphans, ", "),
    "a class no profile throws must be declared an instrument, with its reason"):equals("")
end)

prova.test("an instrument's exemption is real — it stays out of the sweeps on purpose", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "the exemption list is only honest if it is also ACCURATE: an instrument that quietly acquired a profile would be waited on by every commit, which is how an hours-long soak ends up in someone's inner loop",
}, function(t)
  for _, c in ipairs(classes(t)) do
    if INSTRUMENTS[c.name] then
      t:expect(c.orphan, c.name .. " is ad-hoc only, as its exemption claims: " .. c.thrown)
        :is_true()
    end
  end
end)

prova.test("`release` is a superset of `all` — the last look cannot see less", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "the tiers are only a ladder if each rung contains the one below it. A `release` missing a class `all` throws would mean the final gate before a version goes out is WEAKER than the one before a commit — and the failure would show up as a released regression that the pre-commit sweep had already been catching",
}, function(t)
  local thrown_by = {}
  for _, c in ipairs(classes(t)) do
    thrown_by[c.name] = c.thrown
  end

  local missing = {}
  for name, thrown in pairs(thrown_by) do
    if thrown:find("profile `all`") and not thrown:find("profile `release`") then
      missing[#missing + 1] = name
    end
  end
  t:expect(table.concat(missing, ", "), "`release` throws everything `all` does"):equals("")

  -- And it is strictly more, or it would not need to exist as a separate rung.
  t:expect(thrown_by["coverage"] or "", "…plus coverage, which is what makes it the LAST look")
    :contains("profile `release`")
end)

prova.test("the tiers say which question they answer, so the right one is obvious under pressure", {
  covers = "docs/design/manifest.md#switches-not-env-capabilities",
  proves = "a profile list is chosen from in a hurry, and `all` was skipped in favour of a hand-assembled subset precisely because its description read as a size rather than a purpose — the descriptions are the only thing standing between an author and that substitution",
}, function(t)
  local r = shell.run({ prova.bin, "run", "--list" }, { merge_stderr = true, timeout = "60s" })
  t:expect(r.code, "the lanes list renders"):equals(0)
  t:expect(r.stdout, "`all` says when to reach for it"):matches("all%s+before you commit")
  t:expect(r.stdout, "`release` says when to reach for it"):matches("release%s+before you release")
end)
