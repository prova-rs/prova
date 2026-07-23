# prova

**A programmable, language-agnostic acceptance-test runner** — a real scripting language
(Lua) plus a real fixture model, shipped as a single static binary.

Prova is not a unit-test framework (JUnit/pytest own that inside their languages) and not a
single-protocol tool (Hurl owns HTTP). It occupies the **black-box acceptance/integration
layer**: bring a system into existence — render it, build it, boot it — then probe it with
shell + HTTP/gRPC/GraphQL + filesystem + container assertions, with **fixtures** holding
setup and teardown together.

```lua
-- `prova` and the fs/shell/http/docker/… modules are injected globals — no require needed.

local project = prova.fixture("project", Scope.File, function(ctx)
  return archetect.render{
    source = "https://github.com/archetect/archetype-rust-cli.git",
    answers = { project_name = "widget" },
    destination = ctx:tempdir(),
    defaults = true,
  }
end)

prova.test("compiles cleanly", function(t)
  local p = t:use(project)
  t:expect(p:file("Cargo.toml")):exists()
  shell.run("cargo build", { cwd = p.path, check = true })
end)
```

## Install

```shell
brew tap prova-rs/tap
brew install prova
```

Or download a binary for your platform from [Releases](https://github.com/prova-rs/prova/releases)
(linux-x86_64, linux-arm64, macos-arm64, windows-x86_64 + a Windows installer). In CI, use
[`prova-rs/run-action`](https://github.com/prova-rs/run-action):

```yaml
- uses: prova-rs/run-action@v1
  with:
    profile: ci
```

## It teaches itself — to you, and to your agent

Prova is **autodidactic**: the binary carries its own always-current documentation, computed
for *your* package. You (and your coding agent) never have to guess:

```shell
prova init                 # scaffold a package from the archetype catalog, wire IDE support
prova learn                # the topic catalog — one screen per concept, rendered for THIS package
prova learn authoring      # the proof DSL · `pdd` the practice · `running` selection · `project` your shape
prova eval 'return prova.help("shell")'   # every function's signature + summary, from the source stubs
prova skill --install      # drop the agent skill into .claude/skills/ — your agent now knows the loop
prova mcp                  # or serve MCP directly: run/eval/up/introspect/learn as tools
```

The same LuaCATS stubs drive editor completion, `prova.help()`, and the MCP `introspect`
tool — one source, so what your editor shows, what your agent sees, and what runs cannot
drift apart. This is the fastest path to success with prova: scaffold, ask the binary, write
the first proof, and let your agent drive the red→green loop through MCP.

## Why it exists

The existing language-agnostic testers are either **single-domain** (Hurl → HTTP, Bats →
shell) or **declarative YAML/Gherkin** (Venom, Robot Framework, goss). The moment a test
needs a loop, a computed value, a conditional, or reusable scoped setup, YAML hits a wall —
and none of them have a **fixture model**. That's the wedge: a real programming language
*and* pytest-grade fixtures (scoped setup/teardown, dependency injection, caching,
parametrization) in one static binary with no runtime to install.

## What ships today

The engine is built and released (single static binary, no runtime to install):

- **The DSL**: tests, table-driven `test_each`, groups (parallel, isolated), flows (ordered,
  cascade-skip), typed fixture scopes (`Scope.Test/Flow/File/Suite`), a dependency DAG
  (`depends_on`), resource-aware scheduling, `requires` capability gating (skip, never fail)
  and `must_run` environment guarantees (fail, never silently skip).
- **Batteries**: `shell`, `fs`, `net`, `http`, `grpc`, `graphql`, `yaml`, `docker`, `sqlite`
  built in; `http.mock`/`grpc.mock` doubles with request journals; server databases and
  brokers as pinned **plugins** (`require("postgres")`); snapshots with orphan reconciliation.
- **Suites & topologies**: multi-file suites sharing one Lua state (`suite.lua`), whole
  environments addressable by name (`prova up <name>`, warm re-runs while you iterate).
- **Product surface**: `prova.toml` manifest with profiles ([schema](docs/design/manifest.md)),
  surgical selection (`-k`, `--tags`, `--node`, `--last-failed`), a streaming tree console
  with source locations and a failures recap, JSONL/TAP/JUnit reporters, GitHub Actions
  annotations + step summaries out of the box, and a full MCP server mode.

Deep docs, in reading order:

- [`docs/design/foundations.md`](docs/design/foundations.md) — the thesis: orthogonal
  primitives, classic footguns, and how Prova aims to subsume the acceptance-testing landscape
- [`docs/design/proof-driven-development.md`](docs/design/proof-driven-development.md) — the
  practice Prova is an instrument for: agents don't tell you it works, they hand you a proof
  that does
- [`docs/design/api.md`](docs/design/api.md) — the authoring surface: fixtures, assertions,
  the DSL (start here)
- [`docs/design/manifest.md`](docs/design/manifest.md) — the package manifest, every key
- [`docs/design/architecture.md`](docs/design/architecture.md) — the engine
- [`library/prova.lua`](library/prova.lua), [`library/modules.lua`](library/modules.lua) —
  LuaLS annotations: authoritative API surface + editor completion/hover
- [`proofs/`](proofs/) — prova's own acceptance proofs (prova, proven by prova);
  [`examples/`](examples/) — worked suites you can read to feel the ergonomics

## Relationship to archetect

Prova is a sibling to [archetect](https://github.com/archetect), sharing its Lua runtime
and philosophy. The core runner is **domain-agnostic**; archetype rendering is a bundled
**plugin** (`archetect.render` renders in-process via `archetect-core`, no subprocess). It
is both the justifying use case and a dogfooding target: prova renders an archetype, boots
the generated service against ephemeral containers, and holds it to its contract.

## License

MIT
