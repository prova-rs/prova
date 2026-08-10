--- A test's child must never inherit the harness's stdin. tokio's `Command::output()` — unlike
--- std's — only forces stdout/stderr and lets stdin INHERIT, so any child that reads stdin (a
--- journaling shim's `cat`, a credential helper, an interactive prompt) blocks forever whenever
--- prova's own stdin is a non-closing pipe. That was a live 40-minute hang, twice, in the
--- coverage conduct: the cassette-redaction proof's `getcreds` shim sat on the conduct's open
--- stdin. Hermetic contract: shell.run/spawn children see EOF; feeding input is `stdin = ...`.

prova.test("a shell.run child sees EOF, never the harness's open stdin", {
  requires = { "unix" },
  proves = "the getcreds shim's `cat > stdin` under an open harness stdin was a 40-minute wedge — hermeticity is the difference between a conduct and a coin flip",
}, function(t)
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  fs.write(proj .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(proj .. "/proofs/eof_test.lua", [[
prova.test("cat sees EOF instantly", function(t)
  -- Inherits an open pipe -> blocks past its budget (red); hermetic null -> instant EOF.
  local r = shell.run("cat", { timeout = "5s" })
  t:expect(r.code, "cat returned (EOF), not timed out"):equals(0)
end)
]])
  -- Drive the sandbox prova with an OPEN stdin: the sleep holds the pipe's write end for the
  -- whole run, so anything that inherits it cannot see EOF until the sleep dies.
  local r = shell.run("sleep 8 | " .. prova.bin, { cwd = proj, merge_stderr = true, timeout = "30s" })
  t:expect(r.stdout, "the inner suite is green under an open harness stdin"):contains("1 passed, 0 failed")
end)
