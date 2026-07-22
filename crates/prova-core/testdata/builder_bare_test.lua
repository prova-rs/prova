-- MISUSE: a bare `prova.test` inside a flow body. It would register at the file root — outside
-- the flow, unordered, no cascade-skip — so collection must refuse it, not run something else.
prova.flow("wordcount utility", {}, function(flow)
	prova.test("looks like a step, is not one", function(t)
		t:expect(1):equals(1)
	end)
end)
