# project — this package's shape

A prova **package** is rooted where its manifest lives: `prova.toml` or `.prova.toml`, at the
project root or in a `prova/` / `.prova/` child. Discovery walks UP from the working directory,
so run `prova` from anywhere inside the repo. `prova.root` (project root) and `prova.home`
(manifest's directory) are available in every test/eval; anchor repo paths on them, never on cwd.

## This package

{{proof_paths}}

{{plugin_root}}

{{topologies}}

{{profiles}}

{{plugins}}

## The manifest, one line per table

| Key | Meaning |
|---|---|
| `[run] proofs = ["proofs"]` | directory-NAME patterns (not paths): every matching dir below the root holds `*_test.lua` proofs |
| `[run] plugin_root` | THE directory of this package's own plugins; no default — undeclared means none scanned |
| `[run] config` | Lua companion loaded pre-suite (defaults to `prova.lua` beside the manifest) — `runtime.capability` lives there |
| `[run] jobs / format / env` | concurrency (throughput only), output format, run environment |
| `[run] must_run = ["docker"]` | capabilities this environment GUARANTEES — unmet fails the run, never skips |
| `[profiles.<name>]` | overlay on `[run]`, selected with `--profile <name>`; `must_run` unions, the rest overrides |
| `[suites.<name>]` | explicit suite: `paths` share one Lua state (+ optional `setup` file) |
| `[plugins]` | name → source: local path, git URL, `owner/repo@ref`, or `{ git|path, tag|branch|rev, module }` |
| `[sources]` | alias → base (`github:acme`) so plugins can say `"acme:prova-redis@v1"` |
| `[topologies]` | name → a plugin's factory, so `prova up <name>` and proofs address the same environment |
| `[luals] / [updates]` | IDE stub management · git-source freshness policy |

## Where new things go

- **A new proof**: a `*_test.lua` file in any directory matching a `proofs` pattern above.
- **A new local plugin**: a dir under `plugin_root` (`<plugin_root>/<name>/init.lua`); see
  `prova learn init` for `prova init plugin`.
- **A shared fixture/topology**: `prova.topology(name, factory)` in a proof file, or `[topologies]`
  in the manifest when a plugin provides the factory.

Go deeper: `prova learn init` (scaffolding) · `prova learn pdd` (the loop).
