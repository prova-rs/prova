--- `prova introspect [<filter>]` — the CLI twin of the MCP `introspect` tool and `prova.help()`:
--- the API surface (name + signature + one-line summary per function/value), for a human at the
--- terminal and a CLI-driven agent alike. A report (exit 0), never a gate. This is CLI↔MCP parity
--- made concrete: the same discovery capability, the same vocabulary, on both surfaces — paired
--- with `prova learn` (concepts), it is prova's discovery duo (shapes + concepts).

prova.test("prova introspect lists the API surface with names + signatures", function(t)
  local r = shell.run(prova.bin .. " introspect", { merge_stderr = true })
  t:expect(r.code, "a report exits 0"):equals(0)
  t:expect(r.stdout, "a core namespace is present"):contains("shell")
  t:expect(r.stdout, "the tally names the surface"):contains("core API surface")
end)

prova.test("a filter narrows by substring, and the result is smaller than the whole surface",
  function(t)
  local all = shell.run(prova.bin .. " introspect", { merge_stderr = true })
  local narrowed = shell.run(prova.bin .. " introspect shell", { merge_stderr = true })
  t:expect(narrowed.code):equals(0)
  t:expect(narrowed.stdout, "the needle is reflected"):contains("shell")
  t:expect(#narrowed.stdout < #all.stdout, "a filter narrows the surface"):is_true()
end)

prova.test("introspect is a report of at most one filter, no flags", function(t)
  t:expect(shell.run(prova.bin .. " introspect a b", { merge_stderr = true }).code,
    "one filter at a time"):equals(2)
  t:expect(shell.run(prova.bin .. " introspect --nope", { merge_stderr = true }).code,
    "no flags"):equals(2)
end)

prova.test("the binary teaches introspect and advertises it — CLI↔MCP parity for API discovery", {
  proves = "introspect was an agent-only (MCP) capability; giving it a CLI verb means a human and an \
agent reach the same API surface with the same word, and the parity invariants keep the two front-ends \
from drifting into dialects",
}, function(t)
  local learn = shell.run(prova.bin .. " learn introspect", { merge_stderr = true })
  t:expect(learn.code, "learn resolves (learn-per-verb invariant)"):equals(0)
  local help = shell.run(prova.bin .. " --help", { merge_stderr = true })
  t:expect(help.stdout, "the verb is advertised in --help"):contains("prova introspect")
end)
