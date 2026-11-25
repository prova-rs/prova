# quality — lanes, ratchets and standards: warnings handled mechanically

Quality in prova is three pieces, each a proof like any other: **lanes** keep the heavy gates out of
the inner loop, **ratchets** hold a number to a committed floor, and a **standards package** holds a
whole fleet to one bar. Nothing here is advice a reader may skip: each one fails the run.

## Lanes — the heavy gates, behind a switch

A gate that recompiles the workspace (clippy, a coverage build) declares `switch = "<class>"`, so a
bare `prova` never runs it; a profile throws the switch and names the lane:

```toml
[profiles.quality]
proofs   = ["quality"]      # positive selection: proofs/quality/ and nothing else
switches = ["quality"]
```

`prova run quality` runs it; `prova run --list` lists every lane. `must_run` on a profile turns a
missing tool into a failed run, where `requires` on a test only skips it. CI runs the same lanes,
so local and CI cannot drift.

{{profiles}}

## Ratchets — a number that only gets better

```lua
prova.test("production .unwrap() count does not regress", {
  switch = "quality", locks = { prova.writes("cargo") },
}, function(t)
  measure.ratchet(t, "rust.unwrap.production", count, { set = "quality" })
end)
```

The floor lives in `.prova/baselines/<set>.json`, committed and reviewed like code. A regression is
red. A metric with no floor is red too: run `prova run quality --update-baseline` once (that run
reports red while it writes the floors), commit the baselines, and the next run is the green
ratchet. `--update-baseline` only ever tightens; loosening is a hand edit a reviewer sees. Add
`goal` and `deadline` beside a value to schedule a burndown: reaching the goal fails (bank it),
missing the deadline fails (the debt came due). On a legacy codebase the first floors land wherever
the project stands: existing debt becomes ratcheted standing debt, not a wall of red.

## The Rust retrofit

`prova init rust-project` renders the whole surface onto an existing crate or workspace:
`quality` (clippy `-D warnings`, the unwrap/expect census, duplication, file and function sizes),
`ut` (cargo nextest, conducted once and adopted), `coverage`, `all` (the pre-push sweep) and `ci`.
A new warning is then a red test, not a line someone scrolls past. Narrow an intended exception to
the one lint on the one item, with its reason in a comment; a blanket `#![allow(warnings)]` makes
the wall pass by not looking.

## Standards packages — one bar for the fleet

An org publishes its standards as a package of rules, and each repository adopts it with a pin and
one proof file:

```toml
[dependencies]
project-standards = { git = "https://github.com/<org>/prova-project-standards", tag = "v1" }
```

```lua
-- proofs/standards.prova.lua
require("project-standards").check {
  except = { ["cargo/formatted"] = { why = "reformat lands with TICKET-1", ends = "2026-12-31" } },
}
```

Each rule is a test with a stable id and a reason; a waiver is a skip with its reason on every
run, and one past its `ends` goes red again. Raising the bar is: add the rule, prove it can fail,
release, move the floating major tag; every repository answers on its next run (`-U` refreshes a
moved tag). Authoring one: `prova learn package-authoring`.

A red quality lane means fix the code; rebaselining is a decision with its reason in the commit.
Before pushing: `prova run all`.

See also: `prova learn init` (the retrofit) · `prova learn verifiers` (deputed lanes) · `prova learn
running` (switches, profiles) · `prova learn package-authoring` (writing a standards package)
