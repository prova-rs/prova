# specs — proofs authored ahead of implementation

A **spec** is a proof written before the behavior exists. In PDD vocabulary a proof not yet
honored *is* the specification — so the flag is named for what the thing IS, not its state
("pending"). Flag it at the test or flow, with the reason as the value:

```lua
prova.test("json.null encodes an explicit null", { spec = "api-freeze §1" }, function(t)
  t:expect(json.encode({ x = json.null })):equals('{"x":null}')
end)
```

Semantics are xfail-strict, per test:

- **Open spec** (body red) → the distinct `spec` outcome: CI stays green, every reporter names
  it (TAP `# TODO`, JUnit skipped+message, JSONL `"spec"`, console reason + first error line).
- **Spec honored** (body green) → a FAILURE: "spec honored — remove the spec flag from this
  test." An implementation cannot land without deleting its flag in the same commit — the
  finished proof carries no annotation, and there is no cleanup chore later.
- An **unflagged** test holds the line immediately. No drift window exists where a regression
  can hide.
- `spec` is test/flow-level ONLY — on a group or in `suite.config` it is a validation error,
  and `spec = false` is not a thing (an unflagged test is already a full proof).

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
# each spec that turns green now FAILS with "spec honored — remove the spec flag"
# delete that flag in the same commit as its implementation: a proof-carrying change
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
