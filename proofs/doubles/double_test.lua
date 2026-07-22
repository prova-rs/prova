-- Dogfoods prova.double: the transport-agnostic programmable double — the three roles
-- (mock / proxy / spy) plus the ordered event log you assert call sequences against.

local double = require("prova.double")

prova.test("mock: a stub answers, and the call is logged as data", function(t)
  local d = double()
  d:on{ op = "greet" }:reply({ said = "hi" })
  t:expect(d{ op = "greet" }.said):equals("hi")

  local log = d:received()
  t:expect(log):has_length(1)
  t:expect(log[1].source):equals("stub")
  t:expect(log[1].matched):is_true()
  t:expect(log[1].input.op):equals("greet")
end)

prova.test("a :reply function computes from the call and closes over test locals", function(t)
  local calls = 0
  local d = double()
  d:on{ op = "inc" }:reply(function(input)
    calls = calls + 1                         -- a real closure, not a template
    return { total = (input.by or 1) + 100 }
  end)
  t:expect(d{ op = "inc", by = 7 }.total):equals(107)
  t:expect(calls):equals(1)
end)

prova.test("mock: an unstubbed call raises — an unpredicted call is a finding", function(t)
  local d = double{ label = "photoshop" }
  local ok, err = pcall(function() return d{ op = "surprise" } end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):contains("unstubbed call")
  t:expect(tostring(err)):contains("photoshop")   -- the label names the double in the error
  -- ...but it is still recorded, so you can SEE what you failed to predict.
  t:expect(d:received()[1].source):equals("unmatched")
end)

prova.test("proxy: unstubbed calls pass through to the target and are logged; stubs win", function(t)
  local real = function(input) return { echoed = input.msg } end
  local d = double{ target = real }
  d:on{ msg = "override-me" }:reply({ echoed = "STUBBED" })

  t:expect(d{ msg = "override-me" }.echoed):equals("STUBBED")     -- stub wins
  t:expect(d{ msg = "passthrough" }.echoed):equals("passthrough") -- reaches the real target

  local log = d:received()
  t:expect(log[1].source):equals("stub")
  t:expect(log[2].source):equals("target")     -- the proxied one is logged too
end)

prova.test("spy: a target with no stubs is a logging pass-through", function(t)
  local real = function(x) return x * 2 end
  local d = double{ target = real }
  t:expect(d(21)):equals(42)
  t:expect(d:received()[1].source):equals("target")
end)

prova.test("the event log preserves ORDER — assert on the sequence of calls", function(t)
  local d = double()
  d:on(nil):reply(true)                        -- match anything

  d{ step = "open" }
  d{ step = "edit" }
  d{ step = "save" }
  d{ step = "close" }

  local steps = {}
  for _, row in ipairs(d:received()) do steps[#steps + 1] = row.input.step end
  t:expect(steps):equals({ "open", "edit", "save", "close" })
  -- seq is monotonic, so a later assertion can pin an exact position.
  t:expect(d:received()[3].seq):equals(3)
  t:expect(d:received()[3].input.step):equals("save")
end)

prova.test("received(filter) narrows to a subset, ordering preserved within it", function(t)
  local d = double()
  d:on(nil):reply(true)
  d{ kind = "read", key = "a" }
  d{ kind = "write", key = "b" }
  d{ kind = "read", key = "c" }

  local reads = d:received{ kind = "read" }
  t:expect(reads):has_length(2)
  t:expect(reads[1].input.key):equals("a")
  t:expect(reads[2].input.key):equals("c")
end)

prova.test("reset clears the log but keeps the stubs — for phase-by-phase assertions", function(t)
  local d = double()
  d:on(nil):reply("ok")
  d{ phase = 1 }
  d:reset()
  t:expect(d:received()):has_length(0)
  t:expect(d{ phase = 2 }):equals("ok")        -- the stub survived the reset
  t:expect(d:received()[1].input.phase):equals(2)
end)
