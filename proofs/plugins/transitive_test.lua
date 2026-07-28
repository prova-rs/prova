--- A plugin's own `[plugins]` are resolved for its consumer — transitive dependencies.
---
--- The gap this closes was hit building the reference kitchen sink, a plugin whose whole purpose is
--- to COMPOSE others (a topology over postgres + mysql + pulsar). A consumer that pinned only that
--- plugin died with `no prova plugin "postgres"`, because a plugin's own `[plugins]` were read and
--- discarded. Every consumer therefore had to re-declare internals it never mentions and cannot be
--- expected to know — leaking implementation detail into their manifests and breaking the moment the
--- plugin changed what it composes. The information was always there; it simply was not followed.
---
--- Two rules are load-bearing and proven here rather than asserted: an explicit declaration beats a
--- transitive one (a project owns its own environment — a dependency must not swap a version out
--- from under it), and a dependency's relative `path` resolves against the plugin that DECLARED it,
--- not the consumer that pulled it in.
---
--- Note the `vendor/` placement below. Lua's own searcher will load `./<name>/init.lua` relative to
--- the run's directory, so a dependency parked at the consumer's project root is require-able whether
--- or not resolution worked — which silently passes a broken resolver and fails a correct one. These
--- fixtures keep every dependency out of the project root so the only thing under test is the
--- resolver. That subtlety cost a wrong diagnosis before it was noticed.

--- A consumer package that pins `mid`, which in turn pins `leaf` from a vendor directory.
local function nested(t, extra_plugins)
  local root = t:tempdir()

  -- The transitive dependency the consumer never names. Deliberately NOT at the project root.
  fs.write(root .. "/vendor/leaf/init.lua", 'return { who = "vendored-leaf" }\n')
  fs.write(root .. "/vendor/leaf/prova.toml", '[plugin]\nname = "leaf"\n')

  -- mid declares leaf by a path relative to ITSELF (`../vendor/leaf`), which is meaningless
  -- relative to the consumer.
  fs.write(root .. "/mid/init.lua", 'return { who = "mid", leaf = require("leaf").who }\n')
  fs.write(root .. "/mid/prova.toml",
    '[plugin]\nname = "mid"\n\n[plugins]\nleaf = { path = "../vendor/leaf" }\n')

  fs.write(root .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[plugins]\nmid = { path = "./mid" }\n' .. (extra_plugins or ""))
  return root
end

local function run(root)
  return shell.run(prova.bin .. " 2>&1", { cwd = root })
end

prova.test("a consumer inherits the plugins its plugin declares", function(t)
  local root = nested(t)
  fs.write(root .. "/proofs/t_test.lua", [[
    prova.test("leaf resolved without being named", function(t)
      t:expect(require("mid").leaf):equals("vendored-leaf")
    end)
  ]])

  t:expect(run(root).stdout, "the consumer never mentions leaf, yet mid can require it")
    :contains("1 passed, 0 failed")
end)

prova.test("a transitive path resolves against the plugin that declared it", function(t)
  -- `mid` says `../vendor/leaf`. Relative to the CONSUMER that path does not exist; only relative to
  -- `mid` does it name anything. Resolving it at all proves the base directory is the declarer's.
  local root = nested(t)
  fs.write(root .. "/proofs/t_test.lua", [[
    prova.test("leaf is reachable, so the base was mid's own directory", function(t)
      t:expect(require("leaf").who):equals("vendored-leaf")
    end)
  ]])

  t:expect(run(root).stdout):contains("1 passed, 0 failed")
end)

prova.test("an explicit declaration beats a transitive one", function(t)
  -- The consumer pins its OWN `leaf`, shadowing the one `mid` asks for. A project owns its
  -- environment: a dependency cannot quietly substitute a different source.
  local root = nested(t, 'leaf = { path = "./mine" }\n')
  fs.write(root .. "/mine/init.lua", 'return { who = "consumer-leaf" }\n')
  fs.write(root .. "/mine/prova.toml", '[plugin]\nname = "leaf"\n')
  fs.write(root .. "/proofs/t_test.lua", [[
    prova.test("the consumer's leaf wins", function(t)
      t:expect(require("leaf").who):equals("consumer-leaf")
    end)
  ]])

  t:expect(run(root).stdout):contains("1 passed, 0 failed")
end)

prova.test("a dependency cycle terminates instead of looping", function(t)
  -- a → b → a. Nothing about a cycle makes an environment wrong; it only has to stop.
  local root = t:tempdir()
  fs.write(root .. "/vendor/a/init.lua", 'return { who = "a" }\n')
  fs.write(root .. "/vendor/a/prova.toml",
    '[plugin]\nname = "a"\n\n[plugins]\nb = { path = "../b" }\n')
  fs.write(root .. "/vendor/b/init.lua", 'return { who = "b" }\n')
  fs.write(root .. "/vendor/b/prova.toml",
    '[plugin]\nname = "b"\n\n[plugins]\na = { path = "../a" }\n')
  fs.write(root .. "/prova.toml",
    '[run]\nproofs = ["proofs"]\n\n[plugins]\na = { path = "./vendor/a" }\n')
  fs.write(root .. "/proofs/t_test.lua", [[
    prova.test("both sides of the cycle resolve", function(t)
      t:expect(require("a").who):equals("a")
      t:expect(require("b").who):equals("b")
    end)
  ]])

  t:expect(run(root).stdout, "resolution terminates and both are usable")
    :contains("1 passed, 0 failed")
end)
