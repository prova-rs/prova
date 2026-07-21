--- THE PROOF FOR plugin introspection over the CLI (autodidact M4): inside a package that
--- declares a plugin shipping a `library/` stub, `prova.help()` must answer for the plugin's
--- API — the same "one source, N sinks" rail the core stubs ride. Black-box via `prova eval`.
---
--- The launcher (tests/selftest.rs) sets PROVA_BIN and PROVA_FIXTURES.

local prova_bin = assert(os.getenv("PROVA_BIN"), "PROVA_BIN not set")
local fixtures = assert(os.getenv("PROVA_FIXTURES"), "PROVA_FIXTURES not set")
local project = fixtures .. "/mcp-project"

prova.group("plugin introspection (CLI)", function(g)
  g:test("prova.help answers for a declared plugin's stub", function(t)
    local r = shell.run(
      prova_bin .. [[ eval 'local hits = prova.help("greet.hello"); return hits[1] and hits[1].summary or "MISSING"']],
      { cwd = project })
    t:expect(r.code):equals(0)
    t:expect(r.stdout):contains("greeting")
  end)

  g:test("the plugin's API is callable exactly as introspected", function(t)
    -- Introspection is only worth serving if it is TRUE: call what it advertises.
    local r = shell.run(
      prova_bin .. [[ eval 'return require("greet").hello("prova")']],
      { cwd = project })
    t:expect(r.code):equals(0)
    t:expect(r.stdout):contains("hello, prova")
  end)
end)
