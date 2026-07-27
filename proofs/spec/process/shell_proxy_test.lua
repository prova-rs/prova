--- `shell.proxy` — the PATH shim (docs/design/mocks-proxies-drivers.md's interpose posture for
--- the process transport). The SUT shells out to a binary; the shim shadows that name on PATH
--- and interposes: pass through to an upstream and journal (spy), stub selected invocations
--- (stubs always win), or — with no upstream — terminate synthetically, where an unstubbed
--- call is LOUD. The same one-object-three-roles model as prova.double, at the process seam.
---
--- Turn model (feeds the kernel cassette engine): one invocation = argv + stdin → stdout +
--- exit code. Journals speak the §6 spine (seq/source/matched).
---
--- The shim reaches the SUT via `shim.env` — a PATH-prefixed environment handed to whatever
--- spawns the SUT (shell.run here; docker/terminal later).

prova.test("passthrough is a spy — traffic flows to the upstream and the argv is journaled",
  { requires = { "unix" }, proves = "tier-a/shell.proxy: the spy — traffic flows, the turn is journaled" },
  function(t)
  -- Shadow a NON-builtin name: `sh` answers builtins (echo, printf, …) itself without consulting
  -- PATH, so no shim can ever interpose on those — shadow the name the SUT execs, not a builtin.
  local shim = shell.proxy(t, { as = "banner", upstream = "/bin/echo" })

  local r = shell.run("banner hi there", { env = shim.env })
  t:expect(r.code):equals(0)
  t:expect(r.stdout):contains("hi there")        -- the real binary answered

  local e = shim:received()
  t:expect(e):has_length(1)
  t:expect(e[1].seq):equals(1)
  t:expect(e[1].source):equals("target")         -- §6: answered by the upstream
  t:expect(e[1].argv[1]):equals("hi")
  t:expect(e[1].argv[2]):equals("there")
end)

prova.test("a stub overrides the upstream for matching invocations — stubs always win",
  { requires = { "unix" }, proves = "tier-a/shell.proxy: stubs always win; the rest forwards" }, function(t)
  local shim = shell.proxy(t, { as = "git", upstream = "/bin/echo" })
  shim:on{ argv = { "status" } }:reply{ stdout = "clean\n", code = 0 }

  local r = shell.run("git status", { env = shim.env })
  t:expect(r.stdout):contains("clean")
  t:expect(shim:received()[1].source):equals("stub")

  local other = shell.run("git log", { env = shim.env })     -- unstubbed → upstream
  t:expect(other.stdout):contains("log")
  t:expect(shim:received()[2].source):equals("target")
end)

prova.test("no upstream = terminate posture — an unstubbed invocation fails loud",
  { requires = { "unix" }, proves = "tier-a/shell.proxy: no upstream = terminate — unstubbed is loud, like prova.double" }, function(t)
  local shim = shell.proxy(t, { as = "deployctl" })          -- synthetic only
  shim:on{ argv = { "plan" } }:reply{ stdout = "0 changes\n", code = 0 }

  t:expect(shell.run("deployctl plan", { env = shim.env }).stdout):contains("0 changes")

  local boom = shell.run("deployctl apply", { env = shim.env })
  t:expect(boom.code):never():equals(0)                      -- loud, like prova.double
  t:expect(shim:received{ matched = false }):has_length(1)
  t:expect(shim:received{ matched = false }[1].source):equals("unmatched")
end)

prova.test("stdin is part of the turn — journaled and matchable",
  { requires = { "unix" }, proves = "tier-a/shell.proxy: stdin is part of the turn — the process cassette shape" }, function(t)
  local shim = shell.proxy(t, { as = "wc", upstream = "/usr/bin/wc" })

  local r = shell.run("printf 'a b c' | wc -w", { env = shim.env })
  t:expect(r.stdout):contains("3")

  t:expect(shim:received()[1].stdin):equals("a b c")
end)
