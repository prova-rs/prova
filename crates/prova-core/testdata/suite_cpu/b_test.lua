-- The other half of the rendezvous — see a_test.lua for what this pair proves and why it spins
-- rather than sleeps. Deliberately symmetric: whichever worker arrives first waits for the other.
local dir = assert(os.getenv("PROVA_RENDEZVOUS_DIR"), "PROVA_RENDEZVOUS_DIR must be set by the harness")

prova.test("cpu b", function(t)
  fs.write(dir .. "/b.ready", "")
  local deadline = os.clock() + 5
  while not fs.exists(dir .. "/a.ready") and os.clock() < deadline do end
  t:expect(fs.exists(dir .. "/a.ready"), "a_test.lua must be running concurrently"):is_true()
end)
