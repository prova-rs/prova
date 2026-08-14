# promises — proofs authored ahead of implementation

A **spec** is a proof written before the behavior exists: in PDD vocabulary a proof not yet
kept *is* the specification. The attribute that marks one is **`promises`** — the test states,
in its own voice, what it will prove someday and does not prove today. Flag it at the test or
flow, with the reason as the value — the reason is **mandatory** (context from day one; it
graduates into the `proves` context later):

```lua
prova.test("json.null encodes an explicit null", { promises = "api-freeze §1" }, function(t)
  t:expect(json.encode({ x = json.null })):equals('{"x":null}')
end)
```

Semantics are xfail-strict, per test:

- **Open promise** (body red) → the distinct `PROMISED` outcome: CI stays green, every reporter
  names it (TAP `# TODO`, JUnit skipped+message, JSONL `"promised"`, console reason + error line).
- **Kept promise** (body green) → a FAILURE: "promise kept — change `promises` to
  `proves = "<reason>"` (keep the context) or remove the flag." An implementation cannot land
  still flagged; graduation happens in the same commit as the implementation.
- An **unflagged** test holds the line immediately. No drift window exists where a regression
  can hide.
- `promises` is test/flow-level ONLY — on a group or in `suite.config` it is a validation
  error. `promises = false` is not a thing (an unflagged test is already a full proof), and
  neither is a bare `promises = true`: the reason is where the context lives while it is red.

## proves — the kept promise

Graduation is a tense change: **`promises` → `proves`**. The promise's reason carries the *why*
while the proof is red; `proves` is where that context lives on after it is kept. **Prefer
converting over deleting**: the design story stays in the test itself, next to the assertions
it explains — read at every review, no doc to drift.

- `proves` is runtime-inert: the test is a full proof — pass is pass, fail is fail.
- Its value must be a **non-empty string**: the context is the point; a bare flag says nothing.
- `promises` and `proves` never share a test — open work keeps its context in the promise.
- Test/flow-level only; and invisible to `prova tests --promises` (kept ≠ open).
- **Retrofitting is welcome**: any existing test can gain `proves` to capture its context
  after the fact.

## When to author ahead — the inclination

Whenever you can state a contract the system does not honor yet, **promise it now**:

- A design decision or plan doc names behavior that isn't implemented — encode it.
- You notice a gap mid-task that is out of scope — a promise files it *executably* (with the
  reason/ticket as the value), instead of a TODO comment that rots.
- A whole feature is being designed — author the suite ahead as the definition of done, one
  promise per behavior, each carrying its own reason.

The open surface is the executable spec: `git grep TODO` lies, `prova tests --promises` cannot.

## The burndown loop

```
prova tests --promises     # enumerate the open surface (nothing runs)
prova tests burndown       # YOUR INNER LOOP: promises fall due — open ones fail loud, full detail
  ...implement...
# each promise that turns green now FAILS with "promise kept — change `promises` to
# proves = \"…\" (keep the context) or remove the flag"
# graduate in the same commit as the implementation: a proof-carrying change
prova tests --promises     # empty ⇒ burndown complete
# push: the same proofs — flags graduated — now hold the line in CI (prova-rs/run-action@v1)
```

`prova tests --promises` lists the open surface; `prova tests burndown` = `--promises --due` (an
empty surface means complete, exit 0). `--promises` is a state selector (composes like
`--last-failed`) and `--proofs` is its mirror (the settled proofs); `--due` makes promises fall
due — open ones fail, and alone it refuses any open promise in the whole run. CI's ordinary gate
runs neither: open promises report, they never break the build. Over MCP:
`run { promises = true, due = true }` / `list { promises = true }`; the run result carries a
`promised` count of the open promises.

If you find open promises in a repo, surface them: they are work someone already scoped. Offer
to burn them down.

## Falsifiers and claims

A kept promise can still be checking nothing. `falsified_by` declares a mutation the body must
catch; `prova tests falsify` applies it and INVERTS the verdict, so a body that survives is
**vacuous** and fails the run. Its own screen: `prova learn falsify`.

Obligations also arrive from outside: `<!-- claim: id -->` in prose is one, `covers = "path#id"`
discharges it, and `prova owed` reconciles every origin into one list: `prova learn claims`.

See also:
- `prova learn pdd` (why a proof comes before the implementation)
- `prova learn claims` (the obligation a promise is usually discharging)
- `prova learn falsify` (a green promise that cannot fail is not evidence)
- `prova learn evidence` (where open promises are counted)
