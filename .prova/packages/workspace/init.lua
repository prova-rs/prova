-- Structure discovery for the quality gates: everything that needs to know this workspace's
-- shape asks `cargo metadata` — the project's own account of itself — instead of carrying a
-- hardcoded root list. A hardcoded list goes silently stale the day a crate is added (the gate
-- keeps passing while never scanning the new member); the metadata answer cannot.
--
-- Adopted from the `prova init rust-project` archetype's lib package, which generalized this
-- repo's originally hardcoded SRC_ROOTS.

local M = {}

--- `cargo metadata` for this workspace, decoded. Workspace members only (`--no-deps`), so the
--- packages listed are exactly the code this project owns.
M.metadata = prova.fixture("cargo.metadata", Scope.File, function()
  local r = shell.run(
    { "cargo", "metadata", "--no-deps", "--format-version", "1" },
    { cwd = prova.root, timeout = "120s" }
  )
  if r.code ~= 0 then
    error("cargo metadata failed — is this a Rust project?\n" .. (r.stderr or ""))
  end
  return json.decode(r.stdout)
end)

--- The workspace members' source roots (absolute `…/src` dirs), derived from metadata rather
--- than assumed. This is what the file-size and duplication gates scan: production source,
--- never target/ or vendored deps.
---@param meta table a decoded `M.metadata` value
---@return string[]
function M.src_roots(meta)
  local roots, seen = {}, {}
  for _, pkg in ipairs(meta.packages or {}) do
    local dir = pkg.manifest_path:match("^(.*)/Cargo%.toml$")
    if dir then
      local root = dir .. "/src"
      if not seen[root] and fs.exists(root) then
        seen[root] = true
        roots[#roots + 1] = root
      end
    end
  end
  return roots
end

return M
