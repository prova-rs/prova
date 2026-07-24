--- `t:expect(fn):eventually(opts?):<matcher>` — legal only when the subject is a function;
--- re-evaluates `fn` and the terminal matcher until pass or timeout (opts = { timeout, every },
--- defaults shared with prova.retry). On timeout, the failure renders the LAST value seen.

prova.test("polls a function subject until the matcher passes", function(t)
  local n = 0
  t:expect(function()
    n = n + 1
    return n
  end):eventually{ timeout = "5s", every = "10ms" }:gte(3)
end)

prova.test("the k8s idiom — eventually matches a structural subset", function(t)
  local polls = 0
  t:expect(function()
    polls = polls + 1
    return { status = { readyReplicas = math.min(polls, 3) } }
  end):eventually{ timeout = "5s", every = "10ms" }:matches{ status = { readyReplicas = 3 } }
end)

prova.test("opts are optional — retry defaults apply", function(t)
  local flipped = false
  t:expect(function()
    local was = flipped
    flipped = true
    return was
  end):eventually():is_true()
end)

prova.test("a non-function subject is a clear error, not a silent one-shot", function(t)
  local ok, err = pcall(function()
    t:expect(5):eventually{ timeout = "1s" }:equals(5)
  end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):contains("function")
end)
