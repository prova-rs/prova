# prova

**A programmable, language-agnostic acceptance-test runner** — a real scripting language
(Lua) plus a real fixture model, shipped as a single static binary.

Prova is not a unit-test framework (JUnit/pytest own that inside their languages) and not a
single-protocol tool (Hurl owns HTTP). It occupies the **black-box acceptance/integration
layer**: bring a system into existence — render it, build it, boot it — then poke it with
shell + HTTP + filesystem assertions, with **fixtures** holding setup and teardown
together.

```lua
-- `prova` and the fs/shell/http/archetect modules are injected globals — no require needed.

prova.fixture("project", "file", function(ctx)
  return archetect.render{
    source = "https://github.com/archetect/archetype-rust-cli.git",
    answers = { project_name = "widget" },
    destination = ctx:tempdir(),
    defaults = true,
  }
end)

prova.test("compiles cleanly", function(t)
  local p = t:use("project")
  t:expect(p:file("Cargo.toml")):exists()
  local r = shell.run("cargo build", { cwd = p.path })
  t:expect(r.code):equals(0)
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

## Why it exists

The existing language-agnostic testers are either **single-domain** (Hurl → HTTP, Bats →
shell) or **declarative YAML/Gherkin** (Venom, Robot Framework, goss). The moment a test
needs a loop, a computed value, a conditional, or reusable scoped setup, YAML hits a wall —
and none of them have a **fixture model**. That's the wedge: a real programming language
*and* pytest-grade fixtures (scoped setup/teardown, dependency injection, caching,
parametrization) in one static binary with no runtime to install.

## Status

**Design phase.** We are nailing the authoring surface before building the Rust engine.

- [`docs/design/foundations.md`](docs/design/foundations.md) — the thesis: orthogonal
  primitives, classic footguns, and how Prova aims to subsume the acceptance-testing landscape
- [`docs/design/api.md`](docs/design/api.md) — the fixture model + assertion surface (start here)
- [`library/prova.lua`](library/prova.lua), [`library/modules.lua`](library/modules.lua) —
  LuaLS annotations: authoritative API surface + editor completion/hover
- [`examples/`](examples/) — worked acceptance tests you can read to feel the ergonomics

## Relationship to archetect

Prova is a sibling to [archetect](https://github.com/archetect), sharing its Lua runtime
and philosophy. The core runner is **domain-agnostic**; archetype rendering is a first-party
**plugin** (`archetect.render` renders in-process via `archetect-core`, no subprocess). It
is both the justifying use case and our dogfooding target. Two front doors over one core
lib: the standalone `prova` binary, and `archetect test` for authors who already have the
CLI. Library sharing between the repos is TBD as the engine takes shape.

## License

MIT
