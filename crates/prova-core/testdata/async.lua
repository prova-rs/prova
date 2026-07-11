-- Proves the async spine: bodies can await I/O (here, prova.sleep), independent tests overlap
-- concurrently, and a timeout cancels an over-budget body.

prova.test("awaits a sleep", function(t)
  prova.sleep(40)
  t:expect(true):is_true()
end)

prova.test("also sleeps concurrently", function(t)
  prova.sleep(40)
  t:expect(1):equals(1)
end)

prova.test("times out", { timeout = "20ms" }, function(t)
  prova.sleep(200) -- far exceeds the 20ms budget → cancelled and failed
  t:expect(true):is_true()
end)
