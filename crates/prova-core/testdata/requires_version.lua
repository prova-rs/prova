-- Version predicates on capabilities: `requires = { "dotnet >= 9" }`.
--
-- Why this exists, from a real failure: an archetype suite said `requires = { "dotnet" }`, the box
-- had .NET SDK 8.0.421, the archetype targets net9.0 — so the gate said "available", the test ran,
-- and it died on NETSDK1045. A bare name cannot express "dotnet, but 9". The gate answered a
-- question nobody asked, which is the same disease as a readiness probe that cannot fail.
--
-- The vocabulary stays a STRING, deliberately, and that is load-bearing: `must_run` lives in
-- prova.toml (TOML — no functions), so a predicate expressible only in Lua would split the contract
-- into two vocabularies. `requires` (a test's need) and `must_run` (a context's guarantee) must
-- keep parsing the same thing.
--
-- Constraints are semver (`>=`, `<`, `^`, ranges); a probed version is padded to three components
-- (`git version 2.54` → 2.54.0), because tools are inconsistent and the author should not care.

------------------------------------------------------------------------------------------
-- A. Backward compatibility — a bare name is unchanged
------------------------------------------------------------------------------------------

prova.test("a bare capability name still runs", { requires = { "git" } }, function(t)
  t:expect(1):equals(1)
end)

prova.test("a bare missing name still skips", { requires = { "definitely_not_a_real_tool_xyzzy" } },
           function(t)
  error("must never run")
end)

------------------------------------------------------------------------------------------
-- B. The constraint itself
------------------------------------------------------------------------------------------

prova.test("a SATISFIED version constraint runs", { requires = { "git >= 1.0" } }, function(t)
  t:expect(1):equals(1)
end)

prova.test("an UNSATISFIED version constraint skips", { requires = { "git >= 9999.0" } },
           function(t)
  error("must never run — git is not version 9999")
end)

prova.test("whitespace is not significant", { requires = { "git>=1.0" } }, function(t)
  t:expect(1):equals(1)
end)

prova.test("an absent tool with a constraint skips (name fails first, no probe crash)",
           { requires = { "definitely_not_a_real_tool_xyzzy >= 1.0" } }, function(t)
  error("must never run")
end)

------------------------------------------------------------------------------------------
-- C. Operators — semver semantics, not a hand-rolled compare
------------------------------------------------------------------------------------------

prova.test("`<` works", { requires = { "git < 9999.0" } }, function(t)
  t:expect(1):equals(1)
end)

prova.test("an exclusive upper bound skips", { requires = { "git < 0.1" } }, function(t)
  error("must never run")
end)

prova.test("a range works", { requires = { "git >= 1.0, < 9999.0" } }, function(t)
  t:expect(1):equals(1)
end)

------------------------------------------------------------------------------------------
-- D. Platform predicates carry versions too
------------------------------------------------------------------------------------------

prova.test("a bare platform predicate still works", { requires = { "unix" } }, function(t)
  t:expect(1):equals(1)
end)

prova.test("the wrong platform skips before any version probe", { requires = { "windows >= 10" } },
           function(t)
  error("must never run on unix")
end)

------------------------------------------------------------------------------------------
-- E. A version-bearing capability with its own probe
------------------------------------------------------------------------------------------

prova.test("docker's version is probed from the daemon, not the CLI string",
           { requires = { "docker >= 1.0" } }, function(t)
  -- `docker` is a capability with a real probe (the daemon must answer), and its VERSION is the
  -- server's — the thing a suite actually depends on. A generic `docker --version` would report the
  -- client, which can differ from the daemon it is talking to.
  t:expect(1):equals(1)
end)
