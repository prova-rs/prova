--- A proof that finds or reaps its children by command line does it through `strays` (the local
--- package in .prova/packages/strays), whose tokens are this run's own. A fixed process-matching
--- pattern in proofs/ is shared by every prova on the host running the same proof: two gates at
--- once each saw the other's process as a survivor, and one's cleanup killed the other's holder
--- mid-release (prova red at 0325818f, 0f2d1687; D m/11882). So proofs/ holds none.

prova.test("no proof matches processes by a host-wide pattern: strays scopes every probe and reap to this run", function(t)
  -- Built from parts, so this file's own text never matches what it looks for.
  local words = { "pg" .. "rep", "pk" .. "ill", "kill" .. "all" }
  local found = {}
  local files = fs.glob(prova.root .. "/proofs", "**/*.lua")
  table.sort(files)
  for _, path in ipairs(files) do
    local n = 0
    for line in (fs.read(path) .. "\n"):gmatch("([^\n]*)\n") do
      n = n + 1
      for _, w in ipairs(words) do
        if line:find(w, 1, true) then
          found[#found + 1] = path:sub(#prova.root + 2) .. ":" .. n .. ": " .. w
        end
      end
    end
  end
  t:expect(#files, "the census read the proofs"):gt(100)
  t:expect(table.concat(found, "\n"), "use require(\"strays\").alive / .reap with a strays token"):equals("")
end)
