-- Proves the async spine: bodies can await I/O (here, prova.sleep), independent tests overlap
-- concurrently, and a timeout cancels an over-budget body.

prova.test("awaits a sleep", function(t)
  prova.sleep(400)
  t:expect(true):is_true()
end)

prova.test("also sleeps concurrently", function(t)
  prova.sleep(400)
  t:expect(1):equals(1)
end)

prova.test("times out", { timeout = "50ms" }, function(t)
  prova.sleep(2000) -- far exceeds the 50ms budget → cancelled and failed
  t:expect(true):is_true()
end)
