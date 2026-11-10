--- `[isolation]`: every process a proof spawns reaches the run's own roots, never the host's live
--- services (docs/design/manifest.md#isolation). On 2026-11-10 an agora test binary, spawned by its
--- own black-box proof, dialled the host's installed swarm and tried to re-write three real agents'
--- leases; the swarm's guard refused it. A product's tests must not be able to do that by default, so
--- the manifest names the variables a product finds its services by, and Prova sets them for every
--- child to paths under the run's own scratch. Run through `prova.bin` in a sandbox package, so the
--- subject is the build under test, not the prova conducting this suite.

local scaffold = require("scaffold")

local MANIFEST = '[run]\nproofs = ["proofs"]\n\n[isolation]\nunless = "live"\n'
	.. '[isolation.env]\nPROBE_ROOT = "{run}/fabric"\n'

local PROOF = [[
prova.test("a spawned process sees the run's own root", function(t)
  local r = shell.run({ "sh", "-c", 'printf "%s" "$PROBE_ROOT"' })
  fs.write(prova.root .. "/root.txt", r.stdout or "")
end)
]]

local function root_of(proj)
	return fs.exists(proj .. "/root.txt") and fs.read(proj .. "/root.txt") or ""
end

prova.test("every spawned process gets the run's own roots in place of the host's", {
	covers = "docs/design/manifest.md#isolation",
	proves = "a variable [isolation] names reaches each child as a path under this run's own scratch, so a product a proof starts finds a fresh root, never the host's live one, and it does so with nothing written in the proof",
}, function(t)
	local proj = scaffold.package(t, { name = "isolated", manifest = MANIFEST, proofs = { ["probe_test.lua"] = PROOF } })
	local r = shell.run({ prova.bin }, { cwd = proj, merge_stderr = true, timeout = "60s", env = { PROBE_ROOT = "/the/host/live/root" } })
	t:expect(r.code, r.stdout):equals(0)
	local root = root_of(proj)
	t:expect(root, "not the host's value"):never():equals("/the/host/live/root")
	t:expect(root, "under the run's own scratch: " .. root):contains("/run-")
	t:expect(root, "the manifest's path inside it"):contains("/isolation/fabric")
end)

prova.test("the switch [isolation] names turns it off for a live run, and nothing else does", {
	covers = "docs/design/manifest.md#isolation",
	proves = "opting out is a named switch thrown for the run (a live lane), so reaching the host's services is always said, never a default",
}, function(t)
	local proj = scaffold.package(t, { name = "live", manifest = MANIFEST, proofs = { ["probe_test.lua"] = PROOF } })
	local r = shell.run({ prova.bin, "-s", "live" }, { cwd = proj, merge_stderr = true, timeout = "60s", env = { PROBE_ROOT = "/the/host/live/root" } })
	t:expect(r.code, r.stdout):equals(0)
	t:expect(root_of(proj), "the host's own, with the switch thrown"):equals("/the/host/live/root")
end)
