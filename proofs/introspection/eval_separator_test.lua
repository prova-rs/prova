--- `prova eval` can be handed a snippet that begins with a Lua comment
--- (docs/design/agent-ergonomics.md#eval-snippet-starting-with-a-comment).
---
--- The code arrives as ONE argv element, so `--` at position 0 is parsed as a flag. Reporting that
--- as `unknown flag --` names no Lua at all, which sends the author auditing their script instead
--- of the argument boundary — and it bites exactly where snippets are longest, since a long
--- `[==[ … ]==]` block is the one most likely to open with a note about what it does.
---
--- `--` as an end-of-flags separator is the conventional spelling, and this CLI already uses it for
--- `prova lock <token> -- <cmd>`. The teaching error matters as much as the separator: an author
--- who hits this does not know the separator exists, so the failure has to say so.
---
--- Everything drives the SUBJECT through `prova.bin`, never the conducting binary.

local function eval(args)
  local argv = { prova.bin, "eval" }
  for _, a in ipairs(args) do argv[#argv + 1] = a end
  return shell.run(argv, { merge_stderr = true, timeout = "30s" })
end

local COMMENTED = "-- what this snippet is for\nreturn 6 * 7"

prova.test("a snippet opening with a comment runs when passed after `--`", {
  covers = "docs/design/agent-ergonomics.md#eval-snippet-starting-with-a-comment",
  proves = "the separator is the fix, and the assertion has to be that the code RAN — a snippet accepted but silently truncated at its comment would exit 0 and print nothing, which is the same shape of quiet wrongness the argument boundary already caused once",
}, function(t)
  local r = eval({ "--", COMMENTED })
  t:expect(r.code, "the snippet is accepted: " .. r.stdout):equals(0)
  t:expect(r.stdout, "…and the code after the comment actually ran"):contains("42")
end)

prova.test("without the separator, the refusal teaches the separator", {
  covers = "docs/design/agent-ergonomics.md#eval-snippet-starting-with-a-comment",
  proves = "`unknown flag --` is true and useless: the author is holding valid Lua and is told about flags, so they read their script. A real flag is one word — whitespace means this is source, and that is enough to tell the two apart and say the right thing",
}, function(t)
  local r = eval({ COMMENTED })
  t:expect(r.code, "it is still refused rather than guessed at"):equals(2)
  t:expect(r.stdout, "the diagnosis names Lua, not a flag"):contains("Lua")
  t:expect(r.stdout, "…and names the separator that fixes it"):contains("--")
  t:expect(r.stdout, "…and the stdin alternative"):contains("stdin")
end)

prova.test("a genuine unknown flag is still reported as a flag", {
  covers = "docs/design/agent-ergonomics.md#eval-snippet-starting-with-a-comment",
  proves = "the negative control that keeps the heuristic honest: if `--bogus` started reporting itself as Lua, the new message would be worse than the one it replaced — a typo'd flag would send the author looking for a snippet they never wrote",
}, function(t)
  local r = eval({ "--bogus" })
  t:expect(r.code, "an unknown flag is refused"):equals(2)
  t:expect(r.stdout, "…and named as a flag"):contains("unknown flag")
  t:expect(r.stdout, "…not diagnosed as source"):never():contains("looks like Lua")
end)

prova.test("stdin remains the other door, and ordinary snippets are untouched", {
  covers = "docs/design/agent-ergonomics.md#eval-snippet-starting-with-a-comment",
  proves = "a fix to an argument parser earns its keep only if the paths that already worked still do — `-` predates this and is what a caller with arbitrary code in hand should reach for anyway",
}, function(t)
  local piped = shell.run({ prova.bin, "eval", "-" },
    { stdin = COMMENTED, merge_stderr = true, timeout = "30s" })
  t:expect(piped.code, "stdin takes a commented snippet: " .. piped.stdout):equals(0)
  t:expect(piped.stdout, "…and runs it"):contains("42")

  local plain = eval({ "return 1 + 1" })
  t:expect(plain.code, "an ordinary snippet still runs"):equals(0)
  t:expect(plain.stdout, "…and returns"):contains("2")
end)
