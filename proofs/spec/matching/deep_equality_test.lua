--- `equals` answers, even about structures that describe themselves
--- (docs/design/agent-ergonomics.md#equals-must-answer-not-abort).
---
--- Deep equality walks both subjects. A table containing itself describes an infinite structure,
--- and a naive walk follows it until the stack dies — which is not a failed assertion but a dead
--- PROCESS: exit 134, no verdict, no report, and every other test in that run loses its result.
--- The one outcome an assertion must never produce is no outcome.
---
--- Found by writing unit tests for the assertion engine while paying down the coverage floor. No
--- bug report, no failing suite — the defect was reachable from any proof that compared user data
--- with a back-reference in it.

prova.test("a cyclic structure produces a verdict, not a dead runner", {
  covers = "docs/design/agent-ergonomics.md#equals-must-answer-not-abort",
  proves = "before the guard this did not FAIL — it aborted the process with a stack overflow (exit 134), so every other test in the run lost its result too. An assertion's job is to answer; the one outcome it must never produce is no outcome at all. Found by writing coverage tests for the assertion engine, not by a bug report",
}, function(t)
  local a = {}; a.self = a
  local b = {}; b.self = b

  -- Two DISTINCT infinite structures: answered, not walked forever.
  t:expect(a, "two distinct cycles are not equal"):never():equals(b)
  -- The same table IS itself — identity short-circuits before any walk begins.
  t:expect(a, "a cyclic table equals itself"):equals(a)
end)

prova.test("the cycle guard does not change ordinary answers", {
  covers = "docs/design/agent-ergonomics.md#equals-must-answer-not-abort",
  proves = "a depth cap that fired on real payloads would turn equal things unequal — a worse bug than the crash, and a silent one. Real data is shallow; the cap only ever meets a cycle",
}, function(t)
  t:expect({ a = { b = { c = { d = 1 } } } }, "deep nesting still compares")
    :equals({ a = { b = { c = { d = 1 } } } })
  t:expect({ a = { b = { c = { d = 1 } } } }, "…and still discriminates")
    :never():equals({ a = { b = { c = { d = 2 } } } })
end)
