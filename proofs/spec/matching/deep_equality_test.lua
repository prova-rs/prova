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
---
--- BLACK-BOX ON PURPOSE. Written first as `t:expect(a):equals(b)` in this body, which proves
--- nothing about this tree: assertions in a proof body are evaluated by the binary CONDUCTING the
--- suite, so that version tested whichever prova happened to be on PATH. Worse than vacuous — when
--- the conductor was a pre-guard build, the cyclic table killed the CONDUCTOR, and `prova run all`
--- aborted at exit 134 mid-suite, losing 700+ unrelated results (observed 2026-08-15 with an
--- installed 0.22.0). The failure mode here is the process dying, so the proof has to watch a
--- process from outside: run the cyclic comparison in a nested suite under `prova.bin` and assert
--- on its exit code and report.

local CYCLES = [[
  prova.test("a cyclic structure produces a verdict", function(t)
    local a = {}; a.self = a
    local b = {}; b.self = b
    -- Two DISTINCT infinite structures: answered, not walked forever.
    t:expect(a, "two distinct cycles are not equal"):never():equals(b)
    -- The same table IS itself — identity short-circuits before any walk begins.
    t:expect(a, "a cyclic table equals itself"):equals(a)
  end)

  prova.test("the cycle guard does not change ordinary answers", function(t)
    -- A depth cap that fired on real payloads would turn equal things unequal — a worse bug than
    -- the crash, and a silent one. Real data is shallow; the cap only ever meets a cycle.
    t:expect({ a = { b = { c = { d = 1 } } } }, "deep nesting still compares")
      :equals({ a = { b = { c = { d = 1 } } } })
    t:expect({ a = { b = { c = { d = 1 } } } }, "…and still discriminates")
      :never():equals({ a = { b = { c = { d = 2 } } } })
  end)
]]

local function nested(t)
  local dir = t:tempdir()
  fs.write(dir .. "/.prova.toml", '[run]\nproofs = ["proofs"]\n')
  fs.write(dir .. "/proofs/cycles_test.lua", CYCLES)
  return shell.run({ prova.bin }, { cwd = dir, merge_stderr = true })
end

prova.test("a cyclic structure produces a verdict, not a dead runner", {
  covers = "docs/design/agent-ergonomics.md#equals-must-answer-not-abort",
  proves = "before the guard this did not FAIL — it aborted the process with a stack overflow (exit 134), so every other test in the run lost its result too. An assertion's job is to answer; the one outcome it must never produce is no outcome at all. Found by writing coverage tests for the assertion engine, not by a bug report",
}, function(t)
  local r = nested(t)

  -- The load-bearing assertion is about the PROCESS, not the comparison: a runner that died has no
  -- verdict to report, and 134 (SIGABRT) is the signature the unguarded walk left behind.
  t:expect(r.code, "the nested runner must exit with a verdict, not a signal"):equals(0)
  t:expect(r.code):never():equals(134)
  -- The negative control: this string is what a dead runner prints instead of a report. Asserting
  -- its ABSENCE is what makes this proof fail on a regression rather than merely report less.
  t:expect(r.stdout, "a stack overflow is a dead process, not a verdict"):never():contains("stack overflow")
  -- …and the verdict actually arrived. Without this, a runner that silently did nothing passes.
  t:expect(r.stdout):contains("2 passed")
  t:expect(r.stdout):contains("0 failed")
end)
