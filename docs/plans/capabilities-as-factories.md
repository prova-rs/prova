# Plan: capabilities as factories — retiring the companion file

Design ref: `docs/design/capabilities.md` (the durable doc this plan implements). Supersedes the
`runtime.capability` sections of `docs/design/test-topology.md` and the `[run] config` row in
`docs/design/manifest.md`.

## The ask, and the finding that shaped the answer

The complaint was ergonomic: capability predicates lived in a separate Lua companion file with its
own global (`runtime`), its own resolution path (`--config` > `PROVA_CONFIG` > `[run] config` >
`prova.lua`), and no way to test them. Every other declaration in prova — topologies, suites,
dependencies, specs — is either a manifest registration or something in the proof tree. Capabilities
were the odd one out.

The finding that made the change cheap rather than sprawling: **`runtime` had exactly one member.**
`engine/eval.rs` installs `runtime.capability` and nothing else, and `engine/setup.rs`'s
`install_runtime_stub` exists purely to make `runtime.*` a clear error everywhere else. So moving
capabilities into the manifest does not leave a companion behind holding other config — it retires
the file, the global, the stub, the manifest key, the flag, and the env var, together.

Two constraints from the existing code that the new design had to keep rather than argue with:

- **Answers, not closures.** `Capabilities` stores verdicts because mlua handles are `!Send`, each
  suite has its own `Lua`, and `must_run` runs before any suite exists. A "factory" here means
  *addressed by registration*, not *invoked lazily per consumer*.
- **One resolution point.** Checked during planning: outside `capabilities.rs`, the only callers of
  `docker_runs_linux_containers()` are Rust unit tests self-gating (`modules/docker.rs:1384,1443`
  and the `tests/*.rs` guards). The runtime path already funnels through `Capabilities::available`,
  so allowing overrides needed no refactor of the container code — a risk that looked real at design
  time and wasn't.

## Shape

```toml
[capabilities]
"*"     = "error"                                        # fall-through policy: probe | warn | error
docker  = { intrinsic = "docker" }
gpu     = { package = "env", capability = "gpu" }
java    = { command = "java", version = ["-version"], stream = "stderr" }
```

Three selectors (`package` / `command` / `intrinsic`), exactly one per entry. Lua predicates resolve
eagerly at manifest load; `command`/`intrinsic` resolve lazily and memoize per run.

## Steps

1. **Design doc** — `docs/design/capabilities.md`, with claim anchors. Dated bridge item in
   `docs/design/deprecations.md`.

2. **Core: the `Capabilities` model** (`engine/capabilities.rs`). Add declared kinds beside the
   existing eager answers: `probes: BTreeMap<String, CommandProbe>`, `intrinsics: BTreeMap<String,
   String>`, `undeclared: UndeclaredPolicy`, and an `Arc<Mutex<..>>` memo for lazy answers (shared
   across the worker-pool clones, since `RunConfig` is cloned per worker in `suite.rs:319`).
   Resolution order in `available()`/`version()`: declared → built-in → fall-through. `expr_status`
   gains a config-error arm for a refused undeclared name. `regex` becomes an explicit dep of
   prova-core (already in the lock file transitively — a declaration, not new weight).

3. **Core: `resolve_capabilities`** — the entry point that takes manifest declarations and produces
   a `Capabilities`, building a config Lua state only when at least one `package` selector is
   present (so a project with no Lua predicates pays nothing). `load_project_config` stays as the
   deprecation bridge; manifest declarations win on collision and announce the shadowing.

4. **CLI: the manifest section** (`manifest.rs`). `[capabilities]` parses to `BTreeMap<String,
   CapabilityEntry>`, where the entry is either the `"*"` policy string or a declaration table with
   `deny_unknown_fields` and exactly-one-selector validation. Resolve into `Resolved`; wire through
   `suites.rs` (the run path, before `check_must_run`), `cmd_meta.rs` (the report), and
   `mcp/blocking.rs` (the MCP `capabilities` tool).

5. **The report.** `prova capabilities` gains a kind/origin column and marks an overridden built-in
   as overridden. `prova capabilities <name>` explains one: the declaration, the command run, the
   raw output, the parsed version — closing the gap where an unmet capability said only
   `"foo" is unavailable` with no way to see what was probed.

6. **Deprecation bridge.** `warn_once` per companion registration, teaching the replacement TOML;
   `[run] config`, `--config`, `PROVA_CONFIG` keep working until the date in `deprecations.md`.

7. **Dogfood.** Migrate this repo's `prova_selftest` marker out of `.prova/config.lua` into a
   `[capabilities]` declaration, then delete the companion and the `config` key from `.prova.toml`.

8. **Proofs.** Extend `proofs/spec/engine/capabilities_test.lua` and
   `crates/prova-cli/selftest/capability_test.lua`: each selector, the three fall-through policies,
   the built-in override, `version = false`, the intrinsic/declarative equivalence, and the
   deprecation warning firing. Bind them to the new doc's claims with `covers`.

9. **Docs.** Rewrite `topics/capabilities.md` (the `learn` topic), update `topics/authoring.md`,
   `topics/project.md`, `manifest.rs`'s module docs, `docs/design/api.md`,
   `docs/design/test-topology.md`, `docs/design/manifest.md`, and the `library/prova.lua` stub.

## Decisions worth recording

**Strictness governs only names prova does not define.** Forcing `docker = { intrinsic = "docker" }`
under `"*" = "error"` was the first instinct and is wrong: it costs six lines before a strict package
can say `requires = { "unix" }` and buys nothing, since a bare built-in is already nailed down and an
override would be visible in the manifest.

**The policy is a `"*"` entry, not a reserved key or a second section.** A reserved key inside a
name→factory map is the ambiguity prova refuses elsewhere; a whole section for one setting is worse.
Names are `[A-Za-z0-9_-]+`, so `"*"` cannot collide.

**Overrides are allowed now.** The old refusal was protecting against a *silent* override — a
predicate in a file nobody reads. A manifest entry is not silent, and the report names it.

**Not profile-scoped.** Letting a name's meaning vary by profile rebuilds the drift the refusal
prevented, invisibly. `must_run` remains the profile-scoped policy layer.

**No env-var selector.** That is the pattern switches replaced; a declarative kind for it would
rebuild what was torn down.

**No `[[package.capabilities]]` advertisement table.** Designed by analogy with
`[[package.topologies]]`, then dropped during implementation: reading an advertisement requires the
*providing* package's manifest, which would move capability resolution after package resolution —
and `must_run` needs the vocabulary before that. `capability = "gpu"` resolving to `capabilities.gpu`
by convention gives the same encapsulation for free, with `factory = "<dotted.path>"` as the escape
hatch. The analogy was right about the shape and wrong about the cost.

**An undeclared name under `"*" = "error"` fails the run, it does not skip.** Found by running it:
the first implementation routed the config error through `unmet_reason`, which folds everything into
a skip reason — so a closed vocabulary produced a *green* run with an explanatory skip, making
`error` a noisier `warn`. `resolve_requires` now returns `Result` and a config error fails the plan.
That also fixed a pre-existing gap in the same function: its own doc comment promised a malformed
constraint was "an error, not a skip", and it had been a skip all along.
