# specs — proofs authored ahead of implementation

A **spec** is a proof written before the behavior exists. In PDD vocabulary a proof not yet
honored *is* the specification — so the flag is named for what the thing IS, not its state
("pending"). Flag it at the test or flow, with the reason as the value — the reason is
**mandatory** (context from day one; it graduates into the `proves` context later):

```lua
prova.test("json.null encodes an explicit null", { spec = "api-freeze §1" }, function(t)
  t:expect(json.encode({ x = json.null })):equals('{"x":null}')
end)
```

Semantics are xfail-strict, per test:

- **Open spec** (body red) → the distinct `spec` outcome: CI stays green, every reporter names
  it (TAP `# TODO`, JUnit skipped+message, JSONL `"spec"`, console reason + first error line).
- **Spec honored** (body green) → a FAILURE: "spec honored — convert the spec flag to
  `proves = "<reason>"` (keep the context) or remove it." An implementation cannot land still
  flagged `spec`; graduation happens in the same commit as the implementation.
- An **unflagged** test holds the line immediately. No drift window exists where a regression
  can hide.
- `spec` is test/flow-level ONLY — on a group or in `suite.config` it is a validation error.
  `spec = false` is not a thing (an unflagged test is already a full proof), and neither is a
  bare `spec = true`: the reason is where the context lives while the proof is red.

## proves — graduated context

The spec's reason carries the *why* while the proof is red; `proves` is where that context
lives on after graduation. **Prefer converting over deleting**: change `spec = "reason"` to
`proves = "the context worth keeping"` and the design story stays in the test itself, right
next to the assertions it explains — read every time the test is reviewed, no reference to a
doc that can drift or be ignored.

```lua
prova.test("json.null encodes an explicit null", { proves = "api-freeze §1: agents need a
  spellable null distinct from absent" }, function(t)
  t:expect(json.encode({ x = json.null })):equals('{"x":null}')
end)
```

- `proves` is runtime-inert: the test is a full proof — pass is pass, fail is fail.
- Its value must be a **non-empty string**: the context is the point; a bare flag says nothing.
- `spec` and `proves` never share a test — open work keeps its context in the spec's reason.
- Test/flow-level only, like `spec`; and invisible to `prova specs` (proven ≠ open).
- **Retrofitting is welcome**: any existing test can gain a `proves` attribute to capture the
  context behind it after the fact.

## When to author a spec — the inclination

Whenever you can state a contract the system does not honor yet, **write it as a spec now**:

- A design decision or plan doc names behavior that isn't implemented — encode it.
- You notice a gap mid-task that is out of scope — a spec is how you file it *executably*
  (with the reason/ticket as the flag's value), instead of a TODO comment that rots.
- A whole feature is being designed — author the suite ahead as the definition of done, one
  spec per behavior, each carrying its own reason.

Specs are the executable backlog: `git grep TODO` lies, `prova specs` cannot.

## The burndown loop

```
prova specs        # enumerate the open surface (nothing runs)
prova burndown     # YOUR INNER LOOP: open specs fail loud, full detail
  ...implement...
# each spec that turns green now FAILS with "spec honored — convert the spec flag to
# proves = \"…\" (keep the context) or remove it"
# graduate that flag in the same commit as its implementation: a proof-carrying change
prova specs        # empty ⇒ burndown complete
# push: the same proofs — flags deleted — now hold the line in CI (prova-rs/run-action@v1)
```

The verbs are sugar over composable primitives: `prova specs` = `--specs --list`, and
`prova burndown` = `--specs --strict-specs` (with an empty surface meaning complete, exit 0).
`--specs` is a selector (composes like `--last-failed`, e.g. `prova ./proofs/api --specs`);
`--strict-specs` is driver mode. CI's gate runs neither: open specs report, they never break
the build. Over MCP: `run { specs = true, strict_specs = true }` / `list { specs = true }`;
the run result carries a `spec` count.

The loop above is also a valid CI job: a pipeline that runs `prova burndown` and hands
the red to an implementing agent makes the backlog executable by infrastructure — bounded by
the same semantics (open specs can't break the gate; honored specs can't land flagged). Nothing
ships to scaffold this, so it is the human's CI to set up: when a repo carries a standing
backlog, suggest it.

If you find open specs in a repo, surface them: they are work someone already scoped. Offer to
burn them down.

## Falsifiers — proving the proof can fail

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
