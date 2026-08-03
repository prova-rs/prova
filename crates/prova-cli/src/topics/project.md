# project — this package's shape

A prova **package** is rooted at the project ROOT: the directory holding a flat `prova.toml` /
`.prova.toml`, or the PARENT of a `prova/` / `.prova/` nook (which just tucks prova's own files away).
Discovery walks UP from the working directory, so run `prova` from anywhere inside the repo — the nook
included. `prova.root` and `prova.home` are synonyms for that root, available in every test/eval;
anchor repo paths on them, never on cwd.

## This package

{{agent}}

{{proof_paths}}

{{packages_dir}}

{{topologies}}

{{profiles}}

{{packages}}

{{context_files}}

## The manifest, one line per table

| Key | Meaning |
|---|---|
| `[run] proofs = ["proofs"]` | directory-NAME patterns (not paths): every matching dir below the root holds `*_test.lua` proofs |
| `[run] packages` | THE directory of this package's own local packages; no default — undeclared means none scanned |
| `[run] config` | Lua companion loaded pre-suite (defaults to `prova.lua` beside the manifest) — `runtime.capability` lives there |
| `[run] jobs / format / env` | concurrency (throughput only), output format (`console`\|`json`\|`tap`), run environment |
| `[run] color / quiet` | console color (`auto`\|`always`\|`never`) · only failures + recap + tally |
| `[run] github / junit` | GitHub Actions annotations sink (`auto`\|`on`\|`off`) · also write JUnit XML to this path |
| `[run] must_run = ["docker"]` | capabilities this environment GUARANTEES — unmet fails the run, never skips |
| `[profiles.<name>]` | overlay on `[run]`, selected with `--profile <name>`; `must_run` unions, the rest overrides |
| `[suites.<name>]` | explicit suite: `paths` share one Lua state (+ optional `setup` file) |
| `[dependencies]` | name → source: local path, git URL, `owner/repo@ref`, or `{ git|path, tag|branch|rev, module }` |
| `[sources]` | alias → base (`github:acme`) so dependencies can say `"acme:prova-redis@v1"` |
| `[topologies]` | name → a package's factory, so `prova up <name>` and proofs address the same environment |
| `[luals] / [updates]` | IDE stub management · git-source freshness policy |
| `[agent] spec_first` | nudge the agent toward spec-first PDD in `learn project` (default on; `= false` to silence) |
| `context = ["docs/agent.md"]` (top-level) | team docs served as `ctx:<stem>` topics by `prova learn` — the project's own doctrine on this rail |
| `.prova/CONTEXT.md` (a file, not a key) | a zero-config project brief, inlined into `learn project` — drop the file, no manifest entry |

## Where new things go

- **A new proof**: a `*_test.lua` file in any directory matching a `proofs` pattern above.
- **A new local package**: a dir under `[run] packages` (`<packages>/<name>/init.lua`); see
  `prova learn init` for `prova init package`.
- **A shared fixture/topology**: `prova.topology(name, factory)` in a proof file, or `[topologies]`
  in the manifest when a package provides the factory.

Go deeper: `prova learn init` (scaffolding) · `prova learn pdd` (the loop).
