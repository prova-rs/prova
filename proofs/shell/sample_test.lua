--- `shell.run { sample }` (docs/design/verifiers.md#conduct-samples-the-long-lived): a long-lived
--- descendant leaves its stacks and open files before anything kills it.
---
--- Driven through the SUBJECT (`prova.bin eval`), like every conduct-runtime case: called directly,
--- `shell.run` would exercise whichever prova is conducting the suite.

local function subject_eval(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "60s" })
end

--- A script whose NAME is the match, so the sampled process is the one the conduct named.
local function sleeper(dir, seconds)
  local path = dir .. "/prova-sample-target.sh"
  fs.write(path, "#!/bin/sh\nsleep " .. seconds .. "\n")
  shell.run({ "chmod", "+x", path }, { check = true })
  return path
end

prova.test("a matching descendant alive past `after` is sampled once, with its stacks and open files", {
  requires = { "unix" },
  covers = "docs/design/verifiers.md#conduct-samples-the-long-lived",
  proves = "rows 30 and 32 of Substrate's ledger: a test killed at its slow-timeout left nothing but its age, so a stall that never reproduced could never be read",
}, function(t)
  local dir = t:tempdir()
  local script = sleeper(dir, 3)
  local into = dir .. "/samples"
  local r = subject_eval(string.format([[
local r = shell.run({ "sh", "-c", %q }, { sample = { after = "1s", matching = "prova-sample-target", into = %q } })
print("code=" .. r.code)
]], script .. "; true", into))
  t:expect(r.stdout, "the conduct itself is untouched:\n" .. r.stdout):contains("code=0")
  t:expect(r.stdout, "the sample is announced with its path"):contains("prova: sampled pid")
  local files = {}
  for name in shell.run({ "ls", "-1", into }, { check = true }).stdout:gmatch("[^\n]+") do
    files[#files + 1] = name
  end
  t:expect(#files, "one sample for the one long-lived match: " .. table.concat(files, ", ")):equals(1)
  local body = fs.read(into .. "/" .. files[1])
  t:expect(body, "names the process"):contains("prova-sample-target.sh")
  t:expect(body, "carries its stacks"):contains("## stacks")
  t:expect(body, "and its open files"):contains("## open files")
end)

prova.test("a short-lived or non-matching process is never sampled", {
  requires = { "unix" },
  covers = "docs/design/verifiers.md#conduct-samples-the-long-lived",
  proves = "sampling is aimed: the conduct's own long-lived processes (cargo, nextest) and every quick child stay out of the evidence",
}, function(t)
  local dir = t:tempdir()
  local into = dir .. "/samples"
  local r = subject_eval(string.format([[
local r = shell.run({ "sh", "-c", "sleep 2; true" }, { sample = { after = "1s", matching = "no-such-command-zz", into = %q } })
print("code=" .. r.code)
]], into))
  t:expect(r.stdout):contains("code=0")
  t:expect(r.stdout:find("prova: sampled", 1, true), "nothing announced"):equals(nil)
  t:expect(fs.exists(into), "and nothing written"):is_false()
end)

prova.test("a sample with a missing or unknown key is refused, naming the shape", {
  covers = "docs/design/verifiers.md#conduct-samples-the-long-lived",
}, function(t)
  local r = subject_eval([[
print(select(2, pcall(shell.run, "true", { sample = { after = "1s", into = "/tmp/x" } })))
print(select(2, pcall(shell.run, "true", { sample = { after = "1s", matching = "x", into = "/tmp/x", every = "1s" } })))
]])
  t:expect(r.stdout, "a missing key is named:\n" .. r.stdout):contains("sample needs `matching`")
  t:expect(r.stdout, "an unknown key is refused"):contains("every")
end)
