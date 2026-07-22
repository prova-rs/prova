# running — selection is your scalpel

```
prova                        # the whole suite (manifest found by walking up)
prova -k MySQL               # nodes whose path contains MySQL (repeatable; !PAT excludes)
prova --tags '!build'        # by tag, own or inherited (repeatable; !tag excludes)
prova --node "exact › path"  # precisely the node a report named
prova --last-failed          # exactly what was red last run — YOUR MAIN ITERATION VERB
prova --list                 # discover without running (respects selection)
prova <file-or-dir>...       # explicit paths bypass the manifest
```

Selection pulls dependencies in automatically, keeps flows atomic, and never provisions
fixtures for deselected work. Deselected ≠ skipped: the tally says `N deselected`. A selection
matching nothing is an error unless `--allow-empty`.

## Exit codes — what the loop keys on

| Code | Meaning |
|---|---|
| 0 | green (or green-with-skips) |
| 1 | at least one failure |
| 2 | usage error, no tests found, or a `must_run` guarantee unmet (broken environment ≠ skipped test) |

## Output for machines

- `--format json` — JSONL events (node_started/node_finished with outcomes AND the test's
  `file`/`line`) for closing the loop without scraping.
- `--format tap` · `--junit results.xml` (composes with any format; manifest `[run] junit =
  "path"` does the same with no flag) for CI dashboards.
- Inside GitHub Actions, failures auto-emit `::error file=,line=` PR annotations and a
  step-summary table (`--gha off` disables).
- Console output is a tree — file header, group/flow headers, indented leaves with `:line` —
  and a `failures:` recap at the end re-states each failure (full path + `file:line`) with a
  `prova --node "<path>"` rerun line — copy-paste it, don't grep. `-q` = failures (with their
  header chain) + recap + tally only. Color is TTY-only by default (`--color`, `NO_COLOR`).
- `-j/--jobs N` is throughput ONLY — it can never change what a run means.

## Profiles and guarantees

`prova --profile ci` overlays `[profiles.ci]` on `[run]`. A profile's `must_run = ["docker"]`
turns "docker missing → skip" into "docker missing → FAIL": guarantees are what stop a
fully-skipped suite from exiting 0. `prova learn project` shows this package's profiles.

## The rest of the toolbox

- `prova eval '<lua>'` — one-shot probe in the full environment with a real `ctx`; everything
  it provisions tears down on return. `-` reads from stdin (unquotable payloads).
- Snapshots: `-u` rewrites; `--unreferenced warn|delete` on full runs.
- Plugin sources: `-U/--update` force-refresh, `--offline` cache-only.
- Watch mode for topologies: `prova watch <name>` (see `prova learn topologies`).

CI is the same binary and the same suite: `uses: prova-rs/run-action@v1` — byte-identical to
your local run; that is the point.
