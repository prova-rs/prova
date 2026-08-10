--- Black-box spec for the archetect module's EXISTING-FILE POLICY: every write a render performs
--- carries the `if_exists` policy the archetype declared, and the headless driver enforces it.
---
--- The load-bearing case is the DEFAULT, `Existing.Preserve`: it is what makes rendering into a
--- LIVE project safe — retrofit archetypes (`prova init rust-project`) walk into a tree full of
--- files the project owns, and "the project's files always win" is the whole safety story. The
--- terminal driver always enforced this; the headless driver (in-proof `archetect.render`,
--- `prova init`) silently clobbered instead — found live when a retrofit archetype's "your
--- .gitignore survives" proof read back the archetype's own bytes.

local sandbox = prova.fixture("existing-policy-sandbox", Scope.File, function(ctx)
  local root = ctx:tempdir()

  -- One minimal archetype per policy: identical contents (a collidable marker plus a
  -- non-colliding companion), differing only in the `if_exists` the render declares.
  local function archetype(name, policy_arg)
    local dir = root .. "/" .. name
    fs.mkdir(dir .. "/contents")
    fs.write(dir .. "/archetype.yaml",
      '---\ndescription: "existing-policy probe"\nrequires:\n  archetect: "3.0.0"\n')
    fs.write(dir .. "/archetype.lua",
      "local context = Context.new()\n"
      .. 'directory.render("contents", context' .. policy_arg .. ")\n")
    fs.write(dir .. "/contents/marker.txt", "rendered\n")
    fs.write(dir .. "/contents/companion.txt", "companion\n")
    return dir
  end

  return {
    default_policy = archetype("default-policy", ""),
    preserve = archetype("preserve", ", { if_exists = Existing.Preserve }"),
    overwrite = archetype("overwrite", ", { if_exists = Existing.Overwrite }"),
    hard_error = archetype("hard-error", ", { if_exists = Existing.Error }"),
  }
end)

--- A destination whose marker.txt is already owned by the "project".
local function occupied(t)
  local dest = t:tempdir()
  fs.write(dest .. "/marker.txt", "the project's own\n")
  return dest
end

prova.test("the default policy preserves — rendering into a live project never clobbers it", {
  covers = "docs/design/registry.md",
  proves = "retrofit archetypes depend on exactly this: the project's own files win by DEFAULT, while everything non-colliding still lands",
}, function(t)
  local s = t:use(sandbox)
  local dest = occupied(t)
  archetect.render({ source = s.default_policy, destination = dest })
  t:expect(fs.read(dest .. "/marker.txt"), "the project's file survives, byte for byte")
    :equals("the project's own\n")
  t:expect(fs.read(dest .. "/companion.txt"), "the rest of the render still lands")
    :equals("companion\n")
end)

prova.test("Existing.Preserve declared explicitly behaves as the default does", function(t)
  local s = t:use(sandbox)
  local dest = occupied(t)
  archetect.render({ source = s.preserve, destination = dest })
  t:expect(fs.read(dest .. "/marker.txt")):equals("the project's own\n")
  t:expect(fs.read(dest .. "/companion.txt")):equals("companion\n")
end)

prova.test("Existing.Overwrite replaces — an archetype that declares it means it", function(t)
  local s = t:use(sandbox)
  local dest = occupied(t)
  archetect.render({ source = s.overwrite, destination = dest })
  t:expect(fs.read(dest .. "/marker.txt")):equals("rendered\n")
end)

prova.test("Existing.Error fails the render loudly on a collision", function(t)
  local s = t:use(sandbox)
  local dest = occupied(t)
  local ok, err = pcall(function()
    archetect.render({ source = s.hard_error, destination = dest })
  end)
  t:expect(ok, "the render must fail, not resolve the collision either way"):is_false()
  t:expect(tostring(err), "the failure names the collision"):contains("already exists")
  t:expect(fs.read(dest .. "/marker.txt"), "and the project's file is untouched")
    :equals("the project's own\n")
end)
