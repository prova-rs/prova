# Plan: snapshot testing (`matches_snapshot`)

Design refs: `docs/design/api.md` §Snapshots, `docs/design/foundations.md` (snapshot-rot discipline),
roadmap Phase 3 item 7. Stub exists: `Matcher:matches_snapshot(name?)` in `library/prova.lua`.

## Target

```lua
t:expect(rendered_string):matches_snapshot()          -- auto-keyed by test path + counter
t:expect(out:file("src/main.rs")):matches_snapshot("main")   -- named
```
`prova --update-snapshots` (re)writes the `.snap` files; otherwise a mismatch/missing snapshot fails
with a diff and a hint. Snapshots are **reviewable diffs colocated with the test** (insta model).

## Storage & keying

- Location: `<test-file-dir>/snapshots/<test-file-stem>__<key>.snap` (insta-style; colocated, reviewable).
- Key: the explicit `name`, else a slug of the test's node path + a per-test counter (`my-test-1`).
- File format: a small header (source test path) + a blank line + the raw value, so a diff is readable.

## Plumbing (the reason this is bigger than reporters)

1. **File-path tracking.** Collection stamps each node with a `file: usize` index but doesn't retain
   the path. Add `file_paths: Vec<PathBuf>` to the `Collector` (index → source path), populated where
   `current_file` is set (single-file `read_and_collect`, suite `run_suite_files` setup+members).
2. **Thread to the matcher.** `build_plan` looks up `col.file_paths[node.file]`, stores the source dir
   on `PlanItem`; `run_one` sets snapshot context on `TestRun` (dir, key-base, counter) — the `Matcher`
   already holds `run: Rc<RefCell<TestRun>>`, so `matches_snapshot` reads it there.
3. **Update mode.** Add `update_snapshots: bool` to `RunConfig`; thread it into `run_one` → `TestRun`.
   CLI `--update-snapshots` sets it.

## Semantics

- **Match** → assertion passes.
- **Mismatch** (not updating) → fail with a unified-ish diff (expected vs actual) + "run
  --update-snapshots to accept".
- **Missing** (not updating) → fail, write a `.snap.new` next to it (insta parity) so the diff is
  reviewable, hint to update.
- **Update mode** → write `.snap`, pass. (A `.snap.new` is cleaned up.)

## Snapshot levels — the strictness dial (the core design)

A snapshot has a **level** that picks how much of the subject it captures — making the
structure-vs-content choice explicit and defaulting to the safe end:

| Level | Captures | Diff shows | Rot | Use for |
|---|---|---|---|---|
| `layout` | sorted relative paths | files added/removed/moved | low | "the render shape is stable" |
| `content` | paths + each file's bytes | which file changed + line diff | high → keep narrow | a golden file / output |

- Strings & single-file handles → always content (level implicit).
- Tree/dir handles → `level` selects; **default `layout`** (opt into `content`). The anti-rot default
  lives in the API, not in a lint.
- The level is an enum that can grow (`normalized` with redactions, insta-style) without changing call
  sites.

`.snap` formats (diff-friendly): `layout` = a sorted path list; `content` = `=== <path> ===`
delimited sections. On mismatch we print a line-level diff (the powerful part).

## Snapshot protocol (how the matcher stays generic)

The matcher must not know about archetect. Define a protocol: a snapshottable userdata (a tree/file
handle) exposes a method the matcher calls to serialize itself at a level. `matches_snapshot`:
a string subject → its bytes; a handle with the protocol method → call it with the level; else
`tostring`.

## Build sequence

- **Phase A — core (load-bearing).** File-path plumbing (Collector `file_paths` → PlanItem → TestRun),
  `RunConfig.update_snapshots` + `--update-snapshots`, string snapshots colocated as `.snap`, and the
  match / mismatch(diff) / missing(`.snap.new`) / update semantics + the diff renderer. Unit + self tests.
- **Phase B — levels & trees.** The snapshot protocol + `out:tree()` (layout) and file handles
  (content) in `prova-archetect`/`fs`, wiring the `level` option through `matches_snapshot`.
- **Phase C — discipline.** Unused-snapshot detection: a run-wide registry of touched `.snap` files +
  an end-of-run reconcile that flags orphans (foundations' snapshot-rot guard).

## Verify

Unit tests (match / mismatch / missing / update) on a temp dir; a dogfooding self-test
(`--update-snapshots` writes, a second run passes, a tampered value fails). `cargo test` + `clippy` +
LuaLS + run a touched example.
