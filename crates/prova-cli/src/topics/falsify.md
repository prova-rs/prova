# falsify — proving a proof can fail

A proof that has only ever been green is not evidence. It may be checking the contract, or it may
be checking nothing — an assertion over a value that cannot vary, a rule whose subject the
implementation refuses in every case, a bar a stub already satisfies. Those read exactly like a
working proof: same colour, same duration, same line in the report. The difference surfaces in
production, in the thing the suite swore was covered.

`falsified_by` makes the negative case declarable instead of remembered:

```lua
prova.test("no control is anonymous", {
  proves = "accessible and testable are the same property",
  falsified_by = function(t) fs.write(view, unlabelled_button(fs.read(view))) end,
}, function(t)
  t:expect(anonymous_controls(t)):is_empty()
end)
```

```bash
prova tests falsify   # select only tests declaring a mutation; apply it; INVERT the verdict
```

Red under mutation is the passing result — what is being proven is the body's capacity to fail. A
body that survives its falsifier is reported **vacuous** and fails the run:

```
FAIL  two plus two
  ↳ vacuous — the body still passed with its falsifier applied, so it is not asserting what
    the mutation breaks. Sharpen the assertion, or fix the falsifier.
```

## A mutant that hangs — `VACUOUS-HANG`

The mutation this verb most exists to catch is *delete a terminating condition* — and that is
exactly the mutation that produces no verdict at all. So under `falsify` every test is bounded, and
exceeding the bound is its own verdict:

```
FAIL  the watcher stops when the slot settles
  ↳ VACUOUS-HANG — the body did not FAIL under its falsifier, it never returned (no progress
    for 30s). A mutation that removes a terminating condition hangs instead of failing, so this
    is not a surviving mutation and not a falsification either: there is no verdict to invert.
```

It is kin to *vacuous* on purpose — both mean the falsifier told you nothing, one by surviving the
mutation and one by never answering — and it is **never inverted**: a hang is not a red body, so
turning it green would be the strongest possible lie.

The bound, in the order it is resolved:

| | |
|---|---|
| `--timeout <dur>` | run-scoped cap; overrides every test's declared bound. The debug-run lever. |
| `timeout = "30s"` | the test's own wall clock, unchanged and still the ordinary spelling |
| `idle_timeout = "60s"` | the LIVENESS bound — fails only when nothing happens for a window |
| *(falsify default)* | with none of the above, falsify applies its own liveness bound, because a bound you must remember to declare is not a precondition |

`idle_timeout` is the one to reach for when a proof is legitimately slow: it measures *silence*,
not duration, so it can be generous without being useless. Progress at test scope means an
assertion landing — prova captures no test output — so a body doing real work without asserting
looks the same as a wedged one from the outside. That is why it is opt-in everywhere except here.

**What is still unbounded:** a Lua loop that never awaits (`while true do end`). Every bound above
rides the async runtime, so a body that never yields cannot be interrupted yet — see
`docs/design/architecture.md` mechanism 2.

The driver is the selection, exactly like `tests burndown`: a proof without a falsifier is not a
failure (most never declare one), it is simply not what this pass is about. `prova tests falsify` =
`--falsify --allow-empty`; the flag composes with any selection. **A declared falsifier costs nothing on the
ordinary path** — the mutation runs only under the verb that asks for it, because if a bare
`prova` started perturbing systems nobody would ever declare one.

Falsifiers earn their keep where a proof asserts an *absence* — no anonymous control, no leaked
handle, no missing header — because absence is exactly what a broken or vacuous check reports just
as confidently as a working one. Reach for one when a proof has never been seen red, and
especially when the thing under proof was implemented after the proof was written: a stub that
refuses everything satisfies a great many carelessly-written assertions.

A falsifier that raises is reported as a failure of the *mutation*, not of the proof — otherwise a
broken falsifier would masquerade as a body that correctly went red.

`falsified_by` is test-level, like `spec` and `proves`: a group or flow has no assertion of its own
for a mutation to invalidate.

See also: `prova learn promises` (the proofs most needing it) · `prova learn authoring` (where
falsified_by lives) · `prova learn pdd` (why green is not evidence)
