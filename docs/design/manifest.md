# Prova — The Package Manifest (`prova.toml`)

> The authoritative schema reference for the manifest and its resolution rules. The live,
> package-specific rendering of this material is `prova learn project` — that topic computes YOUR
> package's actual proof locations, packages, and topologies at call time; this doc is the durable
> contract behind it. Source of truth in code: `crates/prova-cli/src/manifest.rs` (schema) and
> `crates/prova-cli/src/home.rs` (discovery).

A prova **package** is a directory tree with a manifest. The manifest declares *what to run and
how* — so `prova` with no arguments runs the configured suite, and CI is just `prova`.

## Where the manifest lives (and what "home" means)

Four layouts; the **home** is the project ROOT in every case:

| Manifest file | Home (root) | Shape |
|---|---|---|
| `prova.toml` | the dir holding it | flat — prova at the root |
| `.prova.toml` | the dir holding it | flat, hidden — one file, out of sight |
| `prova/prova.toml` | the dir **above** `prova/` | nested visible — config tucked in a nook |
| `.prova/prova.toml` | the dir **above** `.prova/` | nested hidden |

<!-- claim: paths-resolve-against-home -->
Every manifest-relative path (`proofs`, `config`, `packages`) and generated artifact
(`.luarc.json`, the `.prova/var/` state directory) resolves against the home, never against the
manifest's own directory.

<!-- claim: nearest-manifest-wins -->
Discovery walks **up** from the working directory and the **nearest manifest wins**, so `prova`
runs correctly from anywhere inside the package — including from inside the nook (a bare
`prova.toml` found in a directory named `prova`/`.prova` roots at the parent).

<!-- claim: one-manifest-per-directory -->
Two manifest variants in one directory is an error ("keep exactly one").

<!-- claim: nested-package-isolation -->
A nested manifest deeper in the tree is its own independent package; proof discovery never
crosses into it.

## `[run]` and `[profiles.<name>]`

<!-- claim: profile-overlay-semantics -->
`[run]` is the default profile; `[profiles.<name>]` (selected with `--profile <name>`) overlays
it. Every field is optional. Overlay semantics: a profile field **replaces** the base's when
present — except `env` (base then profile, profile wins per key), `dependencies` (overlaid per
name, profile wins), and `must_run` (**union**, strictly additive: a profile promises *more* than
the package baseline, never less).

<!-- claim: knob-precedence -->
Where a CLI flag or environment variable exists for the same knob, precedence is uniformly
**CLI flag > env var > manifest > auto-detect**.

| Key | Default | Meaning |
|---|---|---|
| `proofs = ["proofs"]` | `["proofs"]` | Directory-**NAME** patterns (basename globs, NOT paths): every matching directory anywhere below the root holds `*_test.lua` / `*.test.lua` proofs. A matched directory owns its whole subtree (no re-matching inside). Discovery skips hidden dirs, `prova`, `target`, `node_modules`, `vendor`, `dist`, `build`, `testdata`, and nested packages. The pattern `"."` is the flat escape hatch: the root itself is a proof dir. |
| `packages` | *none* | THE directory of this package's own packages, root-relative. Deliberately singular, and no default — undeclared means nothing is scanned. Anything from elsewhere gets a name and a pinned source in `[dependencies]`. A profile's value replaces (never adds). |
| `config` | `prova.lua` | The Lua companion loaded once, pre-suite, with the manifest — where `runtime.capability(name, fn)` registers package-wide capability predicates. Path is home-relative. Override per run: `--config PATH` > `PROVA_CONFIG` env > this key. |
| `jobs` | `1` | Concurrent **suites** (`-j/--jobs` wins). Throughput only — it can never change what a run means. |
| `format` | `console` | `"console"` \| `"json"` (JSONL event stream) \| `"tap"`. `--format`/`--json` win. Never auto-switched when piped. |
| `color` | `auto` | `"auto"` \| `"always"` \| `"never"` — console color. `--color` > `PROVA_COLOR` > this key; `auto` additionally honors `NO_COLOR`/`CLICOLOR_FORCE` and never styles a non-terminal. |
| `progress` | `auto` | `"auto"` \| `"always"` \| `"never"` — activity narration during blocking pauses (image pulls, readiness polls, captured commands). **stderr only**, so it can never affect `--format json`/`tap`; that is why it is safe on by default. `--progress` > `PROVA_PROGRESS` > this key, and `--quiet` implies `never` unless `--progress` is explicit. |
| `quiet` | `false` | Only failures (with their header chain), the recap, and the tally. `-q/--quiet` can only *enable*. |
| `github` | `auto` | The GitHub Actions sink: `::error file=,line=` PR annotations + a `$GITHUB_STEP_SUMMARY` table, composing with any `format`. `"auto"` turns on exactly when `GITHUB_ACTIONS=true`. `--gha` > `PROVA_GHA` > this key. |
| `junit` | *none* | Also write a JUnit XML report to this home-relative path (suite named after the package; file/line/timestamp/assertions attributes). `--junit PATH` wins. |
| `[run.env]` | `{}` | Environment applied before the run — the same suite targets ephemeral containers locally and real endpoints in CI by profile-swapping this table. |
| `[profiles.X.packages]` | `{}` | Profile-scoped packages overlaid on package `[dependencies]` (profile wins on a name). The principled home for CI-only capabilities: pinned in-repo, so `--profile ci` resolves identically everywhere. |
| `must_run = ["docker", "dotnet >= 9"]` | `[]` | Capabilities this context **GUARANTEES**, checked as a precondition before anything runs. Same expression grammar as a test's `requires` — but where an unmet `requires` skips, an unmet guarantee **fails the run** (exit 2): a broken environment must not read as a green suite. Unions across base + profile. |

## The other tables

| Table | Meaning |
|---|---|
| `[suites.<name>]` | An explicit suite: `paths = [...]` (dirs/files whose test files all share ONE Lua state — live `Scope.Suite` fixtures) plus optional `setup = "path/suite.lua"`. The zero-config alternative is a directory's own `suite.lua`, which groups that directory's test files (directory-scoped, not the subtree). |
| `[dependencies]` | `name → source`, package-wide. Source forms: a local path string; `"owner/repo@ref"`; or `{ git\|path, tag\|branch\|rev, module }` (`module` defaults to `<name>.lua` then `init.lua`). `require(name)` in any proof resolves through this. |
| `[sources]` | `alias → base` (`github:acme` or a base URL) so packages can say `"acme:prova-redis@v1"`. |
| `[topologies.<name>]` | `package` (required) + exactly one of `topology` (the package's advertised name) or `factory` (dotted path), optional `requires` and `options` (passed to the factory). Sugar for `prova.topology(name, require(package).<factory>)` — the name `prova up <name>` and proofs both address. |
| `[luals]` | `manage = "auto"` (default) \| `"always"` \| `"never"` — the `.luarc.json` pointer policy. `auto` creates when absent and reconciles non-destructively when present (silent in the steady state; a JSONC file it can't parse gets a hint). `never` is the right setting when a repo deliberately commits a hand-maintained `.luarc.json`. Annotations themselves always sync to the shared machine cache. |
| `[updates]` | Git-source freshness for `[dependencies]`: `interval` (default `"1d"`; `"12h"`/`"30m"`/bare seconds), `force` (also `-U/--update`), `retention` (default 90 days for unused materialized trees). `--offline` forbids the network entirely. |
| `context = ["docs/agent.md"]` | **Top-level**, not under `[run]`: team docs (home-relative, `~/` expands) served by `prova learn` as `ctx:<stem>` topics — the project's own doctrine on the same discovery rail. A declared-but-missing file is reported loudly. |

## Worked example

```toml
[run]
proofs   = ["proofs"]           # every proofs/ dir below the root
config   = ".prova/config.lua"  # companion tucked into the nook
jobs     = 4
junit    = "target/prova.xml"

[run.env]
LOG = "info"

[profiles.ci]
jobs     = 8
must_run = ["docker", "dotnet >= 9"]   # CI guarantees these: unmet → FAIL, never skip
[profiles.ci.env]
CI = "true"

[dependencies]
postgres = "prova-rs/prova-postgres@main"

[suites.grpc]
paths = ["services/grpc"]
setup = "services/grpc/suite.lua"

[topologies]
stack = { package = "compose", topology = "stack", options = { file = "docker-compose.yml" } }
```

<!-- claim: empty-resolution-is-an-error -->
An empty resolution (no `proofs` matches and no `[suites]`) is an error, not a silent green run
— as is a `--profile` name that doesn't exist, or a selection that matches nothing (see
`--allow-empty`).

## The package vocabulary, and the spellings it retired

Everything is a package: the project under proof, the libraries it depends on, the topologies
they advertise — one `prova.toml` shape, worn different ways. The manifest says it in four
places: `[package]` is the package declaring itself, `[dependencies]` the packages it consumes,
`[run] packages` the directory of its own local packages, and a `[topologies]` entry names the
`package` providing its factory.

<!-- claim: deprecated-spellings-teach -->
The `plugin` spellings those replaced — `[plugin]`, `[plugins]`, `plugin_root`, `plugin =` in a
`[topologies]` entry, and the `prova plugin`/`prova plugins` verbs — are deprecated, not dead:
each still works for one release and warns once per process naming its successor (the
`spec` → `promises` pattern), and all of them retire together at 1.0. The canonical spellings
warn nothing.
