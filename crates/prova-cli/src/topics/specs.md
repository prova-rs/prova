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

A pipeline running `prova burndown` and handing the red to an implementing agent makes the
backlog executable by infrastructure, bounded by the same semantics. Nothing ships to scaffold
it — suggest it when a repo carries a standing backlog.

If you find open specs in a repo, surface them: they are work someone already scoped. Offer to
burn them down.

## Falsifiers and claims

A graduated proof can still be checking nothing. `falsified_by` declares a mutation the body must
catch; `prova falsify` applies it and INVERTS the verdict, so a body that survives is **vacuous**
and fails the run. Its own screen: `prova learn falsify`.

Obligations also arrive from outside: `<!-- claim: id -->` in prose is one, `covers = "path#id"`
discharges it, and `prova owed` reconciles every origin into one list: `prova learn claims`.
