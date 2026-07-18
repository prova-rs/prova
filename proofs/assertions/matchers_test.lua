-- Dogfoods the matcher surface.
prova.test("core matchers behave", function(t)
  t:expect(2 + 2):equals(4)
  t:expect("prova"):contains("rov")
  t:expect({ 1, 2, 3 }):has_length(3)
  t:expect(nil):is_nil()
  t:expect(5):gt(3)
  t:expect("x"):never():equals("y")
end)

prova.test("gated on the companion capability", { requires = { "prova_selftest" } }, function(t)
  t:expect(true):is_true()   -- runs only because config.lua registered `prova_selftest`
end)
