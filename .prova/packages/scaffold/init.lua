-- The proof-sandbox scaffold (docs/plans/shared-deputies.md increment 3b): recipe sharing via
-- require, applied to the builder every engine proof hand-rolled. Dumb by design — it writes
-- exactly what it is given and defaults only the shape every sandbox shares.
local M = {}

--- Fallback names for unnamed sandboxes. `t:tempdir(key)` is an ACCESSOR — same key, same
--- directory (docs/design/agent-ergonomics.md#context-tempdir-not-idempotent) — so two calls that
--- do not distinguish themselves would land in one place, and that does not fail loudly: the
--- second package overwrites the first and the proof asserts against something it never built.
--- A caller that passes `name` gets a directory that says what it is on disk, which is the whole
--- point; the counter is only for callers who did not bother.
local nth = 0

--- Build an isolated package under `t:tempdir(name)`: a manifest, optional docs/<name> files, and
--- proofs/<name> files. Returns the package root — its own directory, torn down with the scope.
---
--- `opts.name` names the sandbox. Pass it when a test builds more than one: the name reaches the
--- directory's path, so a failure leaves `…-plugin` and `…-consumer` on disk rather than two hex
--- names you have to tell apart by reading the proof.
---
--- The manifest defaults to the bare proofs shape — or, when `docs` are given, to one that also
--- declares them as the spec source (the pairing every specs-lane sandbox wants).
function M.package(t, opts)
  opts = opts or {}
  local key = opts.name
  if not key then
    nth = nth + 1
    key = "pkg" .. nth
  end
  local proj = t:tempdir(key)
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
