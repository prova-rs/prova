--- `prova.bin` — which binary a nested run reaches, and the guard that keeps it honest.
---
--- prova's suites drive prova: a nested run is how a black-box suite asserts on real CLI behavior.
--- *Which* binary that nested run reaches is therefore load-bearing, and `PATH` is the wrong answer
--- to it. One `~/.cargo/bin/prova` is shared by every checkout on the machine, so a bare `prova`
--- inside a proof can be a different build than the one executing that proof — a split nothing in
--- the failure output mentions. It surfaces as a proof failing on a symbol that is demonstrably
--- present in the tree, which is expensive to debug precisely because the evidence in front of you
--- is not wrong; it is answering about a different binary.
---
--- `prova.bin` is the runtime's own executable (`std::env::current_exe`, carried on `RunConfig` so
--- prova-core never assumes the current process is prova). A nested run is then self-consistent by
--- construction: the suite tests the build that is running it, with no environment to arrange and
--- nothing to remember. Note the bound — this guarantees the two layers AGREE, not that either is
--- the local build. Invoke a stale install and both layers are consistently stale; that is a
--- provisioning problem, and `cargo xtask install` names the workspace it replaced for exactly that.
---
--- The third test is the part that lasts. Converting the call sites was a one-time edit; a proof
--- that fails the moment a bare `prova` reappears is what keeps them converted — the same shape as
--- the forbidden-verb list guarding the format rename. It scans every Lua file in the repo, not
--- just this suite, so `crates/prova-cli/selftest/` and anything added later are covered without
--- anyone remembering to extend a list.

prova.test("prova.bin names the executable running this suite", function(t)
  t:expect(prova.bin, "the runtime must inject prova.bin"):is_truthy()
  -- A path, not a bare name — a bare name is precisely the PATH lookup this replaces.
  t:expect(prova.bin:find("[/\\]") ~= nil, "prova.bin must be a path, not a name to resolve"):is_true()
  t:expect(prova.bin):is_file()
end)

prova.test("a nested run through prova.bin executes a real prova", function(t)
  local r = shell.run(prova.bin .. " --version")
  t:expect(r.code, "the injected path must be executable"):equals(0)
  t:expect(r.stdout, "and answer as prova"):matches("prova %d+%.%d+%.%d+")
end)

prova.test("no Lua in this repo reaches prova through PATH", function(t)
  -- Assembled at runtime so this file does not contain the pattern it forbids.
  local via_run = 'shell.run("' .. 'prova'
  local via_spawn = 'shell.spawn("' .. 'prova'

  local offenders = {}
  for _, path in ipairs(fs.glob(prova.root, "**/*.lua")) do
    -- Build artifacts are not sources; vendored copies under target/ are not ours to police.
    if not path:find("/target/", 1, true) then
      local body = fs.read(path)
      if body:find(via_run, 1, true) or body:find(via_spawn, 1, true) then
        offenders[#offenders + 1] = path:sub(#prova.root + 2)
      end
    end
  end

  t:expect(offenders, "these drive prova through PATH — pass through `prova.bin .. \" ...\"` "
    .. "so the nested run is the build under test: " .. table.concat(offenders, ", ")):is_empty()
end)
