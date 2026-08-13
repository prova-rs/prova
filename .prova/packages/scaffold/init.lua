-- The proof-sandbox scaffold (docs/plans/shared-deputies.md increment 3b): recipe sharing via
-- require, applied to the builder every engine proof hand-rolled. Dumb by design — it writes
-- exactly what it is given and defaults only the shape every sandbox shares.
local M = {}

--- Build an isolated package under t:tempdir(): a manifest, optional docs/<name> files, and
--- proofs/<name> files. Returns the package root. The manifest defaults to the bare proofs
--- shape — or, when `docs` are given, to one that also declares them as the spec source
--- (the pairing every specs-lane sandbox wants).
function M.package(t, opts)
  opts = opts or {}
  local proj = t:tempdir() .. "/pkg"
  fs.mkdir(proj .. "/proofs")
  local manifest = opts.manifest
  if not manifest then
    if opts.docs then
      manifest = '[run]\nproofs = ["proofs"]\n\n[[specs.source]]\ntype = "directory"\npath = "docs"\n'
    else
      manifest = '[run]\nproofs = ["proofs"]\n'
    end
  end
  fs.write(proj .. "/prova.toml", manifest)
  if opts.docs then
    fs.mkdir(proj .. "/docs")
    for name, body in pairs(opts.docs) do
      fs.write(proj .. "/docs/" .. name, body)
    end
  end
  for name, body in pairs(opts.proofs or {}) do
    fs.write(proj .. "/proofs/" .. name, body)
  end
  return proj
end

return M
