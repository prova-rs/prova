# Flakes — the ledger and stress mode

A flaky test is a concurrency bug. It is never "load", never "the machine was busy", and it must
never be silently green. Prova therefore treats a flake as a verdict of its own: recorded, counted,
recapped, and — by default — a failing exit.

## The rule

<!-- claim: the-rule -->
A test is **FLAKY** when a failure and a pass are recorded for it on the **same tree digest** and
the **same prova binary** — the `--resume` soundness key (docs/plans/resume.md). Where the two
verdicts come from does not matter:

| source        | how the failure and the pass met                                                    |
|---------------|--------------------------------------------------------------------------------------|
| `rerun`       | an earlier run of the same lane on this tree failed it; this one passed it            |
| `resume`      | a `--resume` re-executed a red leaf and it passed, or `junit.merge` replaced a red case with a re-run pass |
| `junit-retry` | the deputy retried it itself — nextest's `<flakyFailure>`/`<flakyError>`              |
| `stress`      | copies of one `prova stress` run disagreed                                            |

A different tree is a different question: a red on tree A followed by a green on tree B is a fix
(or a change), not a flake. Once flaky on a tree, a test stays flaky for as long as that tree does —
re-running until it happens to pass never clears it; changing the code does.

An execution still passes or fails (`Outcome` is unchanged). Flakiness belongs to the **account**:

- the run record's `executed` map spells the leaf `"flaky"` (`Executed::Flaky`, record schema 5) —
  never `"passed"`, and never evidence: `prova attest` reads a flaky proof as red;
- `summary.flaky` counts the run's flake events (flaky leaves plus flaky deputed cases); a flaky
  leaf is **not** also counted in `summary.passed`;
- a deputed case may carry outcome `"flaky"` (with `flaky_from`, the run that failed it). It is
  never counted red by `junit.verify` and never green: `junit.verify` names it, and the run's
  verdict is `flaky`.

## Detection

Before a run's journal replaces the previous one, prova loads the previous journal of the same lane
— with or without `--resume` — when its tree and binary match this run's, and the previous
`last-run.json` when its tree and binary match. A leaf that passes now and failed there (its journal
row carries the failure's first line) is flaky; a deputed case that passes now and failed there is
re-marked flaky in this run's account. Only unnarrowed lane runs keep a journal, so `-k`/`--node`
inner loops judge only the deputy's own flake marks.

<!-- claim: deputed-cases -->
A deputed case is flaky when the deputy says so (`<flakyFailure>` or `<flakyError>` with no
`<failure>`: it failed, then passed on a retry — the FIRST failure message is kept; a `<failure>`
beside `<rerunFailure>` stayed failed and stays failed), when `junit.merge` replaces a red with a
re-run pass (the merged case is written `<flakyFailure message=…>` with the property
`prova.flaky_from=<run>`), or when an earlier run's record on the same tree failed it.

When a prova leaf conducts a deputy and the deputy's case flaked, both are recorded: the leaf
failed and then passed on the same tree, and so did the case.

## On disk

`.prova/var/flakes.jsonl` — append-only, never truncated, one line per flake event:

```json
{"schema":1,"kind":"flake","test":"widget_test › flaky","deputed":null,
 "tree":"git:…","binary":"3f2a…","failed_run":"run-…","passed_run":"run-…",
 "first_failure":"red on its first execution only","source":"rerun","at":"2026-10-30T12:00:00Z"}
```

`test` is the record key: a file-qualified leaf path, or `suite#name` for a deputed case, whose
identity is also in `deputed: {"verifier","suite","name"}`. `last-run.json` gains
`"verdict": "green" | "flaky" | "red"` and `"flakes": [ …the same events… ]`. Herald reads
`verdict` to report flaky and red separately; these names are a wire format and stay stable.

## Output and exit

The tally shouts it — `12 passed, 0 failed, 1 FLAKY, 0 skipped` — and a recap names each flake with
the run that failed it, the run that passed it, and the line that reproduces it (`prova stress -E
'binary_id(=…) & test(=…)'` for a nextest case, `prova --node …` for a leaf). The GitHub step
summary marks a flaky run ⚠️, never ✅.

<!-- claim: exit -->
`--flakes=fail` is the default: a run whose only blemish is a flake exits 1, because a flake is a
defect. `--flakes=warn` reports it and exits 0. Neither changes the verdict, which is `flaky`.

## Stress mode

<!-- claim: stress-mode -->
`prova stress` reproduces a flake on purpose: each selected nextest case runs as its own process
(`<binary> --exact <name> --nocapture --test-threads=1`, in the suite's cwd, as nextest runs it), N
copies, P at a time, beside M CPU burners.

```text
prova stress [-k PAT]... [--node "<binary-id> <name>"]... [-E FILTERSET]
             [--copies N] [--par P] [--burners M] [--timeout S] [--include-excluded]
             [--listing FILE] [-- <cargo nextest list args>]
```

- **Selection.** `-E` is a nextest filterset, handed to `cargo nextest list`; `-k` keeps a case whose
  `<binary-id> <name>` contains the pattern (case-insensitive, repeatable, `!pat` excludes);
  `--node` names one exactly. Cases the profile's default-filter excludes (a quarantine) are left
  out unless `--include-excluded`; `#[ignore]`d cases always are. A selection that matches nothing
  is refused (exit 2).
- **Built as the gate builds.** The listing — which builds the test binaries — is `cargo nextest
  list --message-format json` plus the args after `--`, or, without them, the manifest's
  `[stress] nextest = [...]` (e.g. `["--workspace", "--features", "cos-daemon/test-daemon",
  "--profile", "prova"]`). A stress binary built with other features is another binary. The listing
  holds the package `cargo` lock, like every prova cargo lane. `--listing FILE` reads a saved
  listing instead.
- **Isolation.** Each copy gets its own SHORT `TMPDIR` under the system temp root (macOS caps a
  Unix socket path at 104 bytes; a long scratch path broke every UDS test), and its own process
  group, killed when the copy ends — so the daemons a test spawned die with it. A copy past
  `--timeout` (default 240 s) is killed and counted a failure.
- **Burners.** `--burners M` starts M `yes > /dev/null` processes, and kills them by PID when the
  run ends — and on SIGINT/SIGTERM, which also kills every live copy's group.
- **Defaults.** `--copies 8`, `--par` = the machine's parallelism, `--burners 0`.

<!-- claim: stress-artifacts -->
A stress run writes `.prova/var/stress/<run>/summary.json`:

```json
{"run":"run-…","tree":"git:…","binary":"3f2a…","copies":8,"par":16,"burners":4,
 "cases":{"cos-daemon::uds tests::bind":{"pass":7,"fail":1,"logs":[".prova/var/stress/run-…/fail/…log"]}}}
```

plus one log per failed copy under `fail/`. A case with both passes and failures appends a
`source: "stress"` line to `flakes.jsonl`. It exits 1 if any copy failed, and writes **no**
`last-run.json`: a stress run is a probe, not a gate run.

A stress red that is green under nextest is a finding to explain, never noise to discard: a copy's
environment differs from nextest's in ways the product may read.
