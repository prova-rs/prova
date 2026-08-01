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
prova falsify      # select only tests declaring a mutation; apply it; INVERT the verdict
```

Red under mutation is the passing result — what is being proven is the body's capacity to fail. A
body that survives its falsifier is reported **vacuous** and fails the run:

```
FAIL  two plus two
  ↳ vacuous — the body still passed with its falsifier applied, so it is not asserting what
    the mutation breaks. Sharpen the assertion, or fix the falsifier.
```

The verb is the selection, exactly like `burndown`: a proof without a falsifier is not a failure
(most never declare one), it is simply not what this pass is about. `prova falsify` = `--falsify
--allow-empty`; the flag composes with any selection. **A declared falsifier costs nothing on the
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
