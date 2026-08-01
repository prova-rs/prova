--- shell.run options that let the portable ARGV form cover what previously forced a shell string:
---   merge_stderr — fold stderr into stdout (retires the `2>&1` redirect, ~35 sites).
---   stdin        — feed the program's input (retires `printf x | cmd`; even `A | B` becomes
---                  `shell.run(B, { stdin = shell.run(A).stdout })`).
--- The subject is prova.bin itself — cross-platform — so these run everywhere, not just under `sh`.

prova.test("merge_stderr folds stderr into stdout — no 2>&1 needed",
  { proves = "increment-2: shell.run { merge_stderr } captures stderr in stdout" }, function(t)
  local r = shell.run({ prova.bin, "eval", 'io.write("OUT\\n"); io.stderr:write("ERR\\n")' },
    { merge_stderr = true })
  t:expect(r.stdout):contains("OUT")
  t:expect(r.stdout):contains("ERR")            -- stderr folded into stdout
  t:expect(r.stderr):equals("")                  -- and no longer on the separate stream
end)

prova.test("stdin feeds the program's input — no pipe needed",
  { proves = "increment-2: shell.run { stdin } feeds stdin" }, function(t)
  local r = shell.run({ prova.bin, "eval", 'io.write(io.read("a") or "")' },
    { stdin = "hello-stdin" })
  t:expect(r.stdout):contains("hello-stdin")
end)
