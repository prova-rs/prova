--- `prova run show <id> --format json` and `prova tree --rev <rev>`, black-box (E m/18306, m/18311, m/18479).
---
--- The contract, in two lines: **a run's record is read back by its id through prova's own reader, wherever
--- prova keeps state (`PROVA_VAR_DIR` too), in a versioned shape**; and **a revision's tree is fingerprinted
--- exactly as a run records it, from that revision's tracked files read through git, never the worktree.**
--- Together they let a consumer (Themis) tell a run of the break from a run of the fix without carrying a
--- copy of prova's digest or its file layout.

local sandbox = prova.fixture("run-show-sandbox", Scope.Test, function(ctx)
  local root = ctx:tempdir()
  local pkg = root .. "/pkg"
  fs.mkdir(pkg .. "/proofs")
  fs.write(pkg .. "/.gitignore", "/.prova/var/\n")
  fs.write(pkg .. "/prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(pkg .. "/proofs/widget_test.lua", [[
prova.test("arithmetic holds", function(t) t:expect(1 + 1):equals(2) end)
prova.test("the widget is finished", function(t) t:expect("not yet"):equals("finished") end)
]])
  local git = function(args)
    shell.run(args, {
      cwd = pkg,
      check = true,
      env = { GIT_AUTHOR_NAME = "t", GIT_AUTHOR_EMAIL = "t@t", GIT_COMMITTER_NAME = "t",
        GIT_COMMITTER_EMAIL = "t@t" },
    })
  end
  git({ "git", "init", "-q" })
  git({ "git", "add", "-A" })
  git({ "git", "commit", "-q", "-m", "one" })
  return { root = root, pkg = pkg }
end)

local function run(dir, args, var_dir)
  return shell.run(prova.bin .. " " .. (args or ""), {
    cwd = dir,
    env = { PROVA_VAR_DIR = var_dir or "" },
  })
end

--- The id of the run `PROVA_VAR_DIR=var_dir prova` just kept, read from where prova put it.
local function kept_id(base)
  local found = fs.glob(base, "**/runs/run-*.json")
  if #found ~= 1 then
    return nil
  end
  return found[1]:match("(run%-%d+%-%d+)%.json$")
end

prova.test("a red run is shown by its id, its rows and the tree it ran on in a versioned shape", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local id = kept_id(sb.pkg .. "/.prova/var")
  t:expect(id ~= nil, "the red run was kept by id"):is_true()
  local r = run(sb.pkg, "run show " .. id .. " --format json")
  t:expect(r.code, r.stderr):equals(0)
  local shown = json.decode(r.stdout)
  t:expect(shown.schema):equals(1)
  t:expect(shown.id):equals(id)
  t:expect(shown.verdict):equals("red")
  t:expect(shown.tree, "the tree it ran on, as prova fingerprints it"):matches("^git:%x+$")
  local by = {}
  for _, row in ipairs(shown.rows) do
    by[row.name] = row.outcome
  end
  t:expect(by["widget_test › the widget is finished"], "the red row"):equals("failed")
  t:expect(by["widget_test › arithmetic holds"], "and the green one"):equals("passed")
end)

prova.test("run show reads a run kept under PROVA_VAR_DIR, as the run wrote it", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  local state = sb.root .. "/state-root"
  run(sb.pkg, nil, state)
  local id = kept_id(state)
  t:expect(id ~= nil, "the run was kept under the override"):is_true()
  local plain = run(sb.pkg, "run show " .. id .. " --format json")
  t:expect(plain.code, "without the override it is not there"):never():equals(0)
  local moved = run(sb.pkg, "run show " .. id .. " --format json", state)
  t:expect(moved.code, moved.stderr):equals(0)
  t:expect(json.decode(moved.stdout).id):equals(id)
end)

prova.test("a run prova doesn't hold is refused, naming the id and where it looked", {}, function(t)
  local sb = t:use(sandbox)
  local r = run(sb.pkg, "run show run-1-1 --format json")
  t:expect(r.code):never():equals(0)
  t:expect(r.stderr):contains("run-1-1")
  t:expect(r.stderr):contains(".prova/var")
  t:expect(r.stdout, "nothing a reader could take for a record"):equals("")
end)

prova.test("tree --rev prints the fingerprint a run at that revision records, and ignores the worktree", {
  requires = { "git" },
}, function(t)
  local sb = t:use(sandbox)
  run(sb.pkg)
  local id = kept_id(sb.pkg .. "/.prova/var")
  local recorded = json.decode(run(sb.pkg, "run show " .. id .. " --format json").stdout).tree
  local at_head = run(sb.pkg, "tree --rev HEAD")
  t:expect(at_head.code, at_head.stderr):equals(0)
  t:expect(at_head.stdout:gsub("%s+$", ""), "the same digest the run recorded"):equals(recorded)

  -- Dirty the worktree: the revision's fingerprint never moves, the worktree's does.
  fs.write(sb.pkg .. "/proofs/widget_test.lua", "-- edited\n")
  fs.write(sb.pkg .. "/new_test.lua", "-- untracked\n")
  t:expect(run(sb.pkg, "tree --rev HEAD").stdout:gsub("%s+$", ""), "a dirty worktree"):equals(recorded)
  run(sb.pkg)
  local rerun = json.decode(fs.read(sb.pkg .. "/.prova/var/last-run.json")).tree
  t:expect(rerun, "while a run of the edited worktree records another tree"):never():equals(recorded)
end)

prova.test("tree --rev refuses a revision git doesn't have, by name", { requires = { "git" } }, function(t)
  local sb = t:use(sandbox)
  local r = run(sb.pkg, "tree --rev no-such-rev")
  t:expect(r.code):never():equals(0)
  t:expect(r.stderr):contains("no-such-rev")
  t:expect(r.stdout):equals("")
end)
