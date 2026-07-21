-- Proves this project consumes the EXTERNAL `parallels` plugin — pinned in .prova.toml [plugins] as a
-- git source (prova-rs/prova-parallels), fetched into the cache, and `require`d here exactly as any
-- consumer would. The plugin carries its own self-test; this is the consumer's end of the extraction:
-- the git-sourced plugin loads, advertises `vm`, and gates on `prlctl`. The live VM provisioning is
-- behind `requires = { "prlctl" }`, so it runs only on a Parallels host and skips cleanly elsewhere.
local parallels = require("parallels")

prova.test("the parallels plugin loads and exposes its `vm` factory", function(t)
	t:expect(type(parallels.vm)):equals("function")
end)

prova.test("the `vm` factory demands a base image to clone", function(t)
	-- No `image` → a clear error, not a confusing prlctl failure. Hermetic: this never reaches prlctl.
	local ok, err = pcall(parallels.vm, t, {})
	t:expect(ok):equals(false)
	t:expect(tostring(err)):contains("image")
end)

-- The real thing, gated on Parallels. It needs a base template to clone — machine-specific, so it
-- comes from PROVA_PARALLELS_IMAGE. Absent either `prlctl` or the template name, it SKIPS (never a
-- spurious failure): with both, it linked-clones a VM, execs in the guest, asserts it's Linux, and
-- tears the clone down with the test.
prova.test("provisions a VM and drives the guest", { requires = { "prlctl" } }, function(t)
	local image = os.getenv("PROVA_PARALLELS_IMAGE")
	if not image then
		t:skip("set PROVA_PARALLELS_IMAGE to a base VM template name to exercise real provisioning")
		return
	end
	local vm = parallels.vm(t, { image = image })
	t:expect(vm:run({ "uname", "-s" })):contains("Linux")
	t:expect(vm:ip()):never():equals("")
end)
