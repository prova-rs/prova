-- prova.workspace — a scratch directory tied to a test/fixture scope.
--
-- A bundled first-party plugin, loaded through the same searcher user plugins use (proving the
-- loadable path). Composes existing primitives only: `fs` + `ctx:manage`. Follows the plugin
-- contract — one namespace table, `(ctx, opts)` context-first, lifecycle via `ctx:manage`
-- (the handle exposes `close`, so teardown removes the directory automatically).
--
--   local workspace = require("prova.workspace")
--   local ws = workspace.create(ctx)
--   ws:write("src/main.rs", "fn main() {}")
--   assert(ws:exists("src/main.rs"))

local workspace = {}

--- Create a scratch directory whose lifetime is tied to `ctx`. Removed on scope teardown.
--- @param ctx any  the fixture/test context (needs `:manage`)
--- @param opts table|nil  reserved for future options
--- @return table  { path, file(rel), write(rel, contents), read(rel), exists(rel), close }
function workspace.create(ctx, opts)
  assert(ctx and ctx.manage, "workspace.create(ctx, opts?): pass the fixture/test context first")
  local _ = opts

  local ws = { path = fs.tempdir() }

  function ws:file(rel)
    return self.path .. "/" .. rel
  end

  function ws:write(rel, contents)
    local p = self:file(rel)
    fs.write(p, contents)
    return p
  end

  function ws:read(rel)
    return fs.read(self:file(rel))
  end

  function ws:exists(rel)
    return fs.exists(self:file(rel))
  end

  -- `ctx:manage` calls this on teardown (the handle has `close`, so it counts as a managed resource).
  function ws:close()
    fs.remove_all(self.path)
  end

  return ctx:manage(ws)
end

return workspace
