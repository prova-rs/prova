prova.test("cpu a", function(t)
  local sum = 0
  for i = 1, 30000000 do sum = sum + i end
  t:expect(sum > 0):is_true()
end)
