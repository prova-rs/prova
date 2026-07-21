# Plan — de-conflate `prova init`: a catalog-driven scaffolder + `prova ide setup`

Status: **shipped** (M0–M6, 2026-07-20). Proof-Driven Development from the first line: proofs went
red before implementation. What actually landed, and where it refined this plan:

- **`prova ide setup`** split out (M1); **catalog** model + `--list` + built-in `default` (M2);
  **`prova init <key>` renders** the selected archetype via `prova_archetect::render_interactive`
  with `--answer` / `--switch` / `--defaults` / `--headless` (M3, precedence includes M4's baked
  answers); **keyless `prova init` picks interactively** via inquire, non-TTY → clear error (M6).
- **Built-in `default` source** is the git repo `…/prova-init-default-archetype.git#main` (not
  embedded; `#main` until a `v1` tag is cut).
- **No `--force`** (dropped M5): overwrite is the archetype author's + init-entry's concern, not a
  prova flag. The never-clobber guard stays.
- **Flag:** `--no-ide` is the name; `--no-luals` kept as an alias. `--hidden`/`--flat` are gone —
  layout is the archetype's job.
- The default archetype no longer prompts for author (unnecessary for a proof-suite scaffold).

The original plan follows, for the design rationale.

## The problem

`prova init` currently does two unrelated jobs in one verb (`crates/prova-cli/src/init.rs`):

1. **Scaffold** a package — create the home dir, write a *hardcoded* `manifest_template()`, print
   "next" guidance.
2. **Wire the IDE** — install the shared LuaLS core stubs and create/merge `.luarc.json`
   (`annotations::init`).

Two problems fall out of the conflation:

- The scaffold is a single frozen string in the binary. There is exactly one shape of package prova
  can create, and changing it means editing Rust. Prova already renders archetypes in-process
  (`prova-archetect`) — the scaffold should *be* one.
- IDE wiring is a distinct, re-runnable concern (regenerate machine-local `.luarc.json` after a
  clone; a Lua-native project that only wants annotations). It doesn't belong welded to first-time
  scaffolding.

## The shape we want

Split the verbs, and make `init` a **catalog** over archetypes that prova renders with
archetect-core — the same embeddable library the whole ecosystem is stress-testing.

```
prova init                     # interactive: pick from the catalog (inquire select), then render
prova init default             # render the `default` entry into the current project
prova init archetype           # render a different entry (e.g. scaffolding for authoring archetypes)
prova init <key> [--answer k=v]... [--switch s]... [--defaults] [--headless] [--force]
prova init --list              # print catalog keys + descriptions (scriptable; no prompt)

prova ide setup                # (was folded into init) install core stubs + create/merge .luarc.json
prova ide setup [--manage auto|always|never]
```

`init` renders **into the current project directory** (`.`) — it scaffolds a test suite in the repo
you're standing in, it does not create a new project sub-directory. After a successful render it
finishes by running the IDE wiring (unless `--no-ide`), preserving today's one-command bootstrap.

### Decisions locked (2026-07-20)

- **Built-in `default` entry.** prova ships a catalog with a `default` mapping so `prova init` /
  `prova init default` works out of the box with zero user config. User config *extends and
  overrides* it by key.
- **Keep the clobber guard, add `--force`.** `init` still refuses when the package is already
  initialized (a `prova.toml` exists at any of the four known locations); `--force` opts into
  re-rendering over it.
- **Interactive catalog via inquire.** No-argument `init` presents the catalog with
  archetect-terminal-io's `inquire`-based select (key + description), consistent with the archetype
  prompts that follow. Non-TTY invocation degrades to a clear error that names `--list` / `init
  <key>` rather than hanging.

## The catalog: `~/.config/prova/config.toml`

`SystemLayout::config_dir()` already exists and is commented *"global `prova.toml` defaults
(future)"* — `~/.config/prova/config.toml` is its first real inhabitant. It declares init entries,
each mapping a key to a description, an archetype source, and optional baked answers/switches. This
is prova's answer to an archetect catalog, owned by prova so prova renders the selection UI itself.

```toml
# ~/.config/prova/config.toml

[init.default]                       # overrides the built-in `default`
description = "A standard prova package — a proof suite (proofs/ + .prova/)"
source      = "https://github.com/prova-rs/prova-init-default-archetype.git#v1"
# Everything below is optional.
switches    = ["ci"]                 # archetype switches always passed for this entry
defaults    = false                  # take the archetype's default for any unanswered prompt

[init.default.answers]               # baked answers — never prompted, always supplied
proof_dir = "proofs"

[init.archetype]                     # a second entry, purely additive
description = "Scaffolding to author + test a prova archetype"
source      = "https://github.com/prova-rs/prova-init-archetype-archetype.git#v1"

[init.service]
description = "A service package pre-wired for postgres + http"
source      = "/Users/me/archetypes/prova-service"   # local path source works too
```

A `source` is anything `prova-archetect` can already resolve: a git URL (optionally `#ref`) or a
local path.

### The author's three modes for a parameterized answer

This is the motivating example: an archetype that parameterizes the proof-suite directory name. The
config author chooses, per key, how that answer is supplied:

| Mode | config.toml | Behavior |
|---|---|---|
| **Baked** | `answers.proof_dir = "proofs"` | Never prompted; always `proofs`. |
| **Prompted** | omit it | The archetype prompts each `init` (respecting its own default). |
| **Ad-hoc** | omit it; user passes `--answer proof_dir=tests` | Supplied at the CLI for this run. |

### Answer resolution precedence (highest wins)

1. CLI `--answer key=value` (repeatable)
2. `[init.<key>].answers` baked in config.toml
3. Interactive prompt (archetype default pre-filled) — unless `--headless`
4. `--defaults` / `defaults = true`: take the archetype's default without prompting
5. `--headless` with no answer and no default → **hard error** (never hang)

`--switch` (CLI, repeatable) unions with `[init.<key>].switches`.

### Catalog merge

Built-in catalog (embedded in the binary) is the base map. `~/.config/prova/config.toml`'s
`[init.*]` tables are layered on top: a matching key **replaces** the built-in entry; a new key
**adds** one. So a user can redefine `default` or add `service`, and `--list` reflects the union.

## Rendering: interactive, in-process

`prova-archetect` today renders **headlessly** (`render_headless`, `CapturingIoHandle`) for the
in-test `archetect.verify{...}` path. `init` needs the interactive sibling. archetect ships exactly
the driver for it: `archetect-terminal-io::TerminalScriptIoHandle`, an `inquire`-backed
`ScriptIoHandle` that prompts only for what pre-seeded answers / `use_defaults` don't already cover.

Add to `prova-archetect`:

```rust
pub fn render_interactive(
    source: &str,
    destination: &Path,
    answers: ContextMap,
    switches: Vec<String>,
    defaults: bool,     // .with_use_defaults_all — undefaulted prompts still prompt
    headless: bool,     // Configuration::with_headless — undefaulted prompts error, never prompt
) -> Result<Vec<String>, ArchetectError>
```

Differences from `render_headless`:

- **Driver:** `TerminalScriptIoHandle` (a new `archetect-terminal-io` dependency on the crate) when
  interactive; the headless `CapturingIoHandle` when `--headless`.
- **System layout:** archetect's **real XDG layout**, not the per-process temp layout the test
  render uses — so an archetype's `catalog:` library deps (`author-prompts-library`,
  `gitignore-library`) resolve and cache in the normal place, shared with a real archetect install.
- **Thread:** run on the calling thread. `init` has no Tokio runtime to isolate from, and
  interactive prompts need the real stdin/stdout; the OS-thread hop that `render_headless` uses to
  escape prova's worker runtime is unnecessary here.

### The built-in default: hermetic + offline

To keep `prova init` working with no network and version-locked to the binary, **embed the
`prova-init-default-archetype/` directory** into the binary (`include_dir!`), extract it to a temp
dir on demand, and render from that local path. The archetype's remote `catalog:` libraries still
fetch on first render (then cache) — that's archetect's normal behavior and acceptable. User-defined
entries always render from their declared `source`.

## Command-surface migration

The old scaffold-shape flags describe a layout the *archetype* now owns, so they move:

| Old | New |
|---|---|
| `prova init --hidden` / `--flat` | an **answer** the default archetype takes (e.g. `--answer layout=hidden`), or a distinct catalog entry |
| `prova init --no-luals` | `prova init --no-ide` (skip the finishing IDE step), or `prova ide setup --manage never` |
| `manifest_template()` in `init.rs` | content of `prova-init-default-archetype/contents/` |

## Crate + file changes

- **`prova-archetect`**
  - add `archetect-terminal-io` dep (pinned to the same `v3.1.1` tag as core/api).
  - add `pub fn render_interactive(...)` (above). Keep `render_headless` untouched for the test path.
- **`prova-cli`**
  - `catalog.rs` *(new)* — `Catalog { entries: BTreeMap<String, InitEntry> }`; embedded built-in
    base; parse + merge `~/.config/prova/config.toml`; `InitEntry { description, source, switches,
    defaults, answers }`.
  - `ide.rs` *(new)* — `prova ide setup`, wrapping `annotations::init` / `setup`. `main.rs` gains an
    `ide` subcommand branch beside `init`.
  - `init.rs` *(rewrite)* — parse `<key>` + `--answer/--switch/--defaults/--headless/--force/--list/
    --no-ide`; resolve the catalog; interactive `inquire` select when no key; clobber guard; dispatch
    to `render_interactive` into `.`; finish with `ide::setup` unless `--no-ide`. Delete
    `manifest_template()`.
  - embed `prova-init-default-archetype/` via `include_dir!`.

## PDD approach — proofs first, red before green

Consistent with `docs/design/proof-driven-development.md`: write the black-box proof, watch it fail,
implement to green, never weaken a proof to pass it. CLI/filesystem/interactive behavior is proven
the way the existing suite proves it — **Rust integration tests through `CARGO_BIN_EXE_prova`**
(cf. `tests/eval_cli.rs`, `tests/config_key.rs`) — plus an optional dogfood `proofs/init/*_test.lua`
that shells out, for the "prova proves prova" story.

Determinism rule: proofs drive the **non-interactive** paths (`--answer`, `--defaults`,
`--headless`, `--list`, baked answers). `inquire` needs a TTY, so the interactive select is proven
only at its edges (catalog enumeration via `--list`; graceful non-TTY error), plus a thin unit test
on selection, not an end-to-end keystroke test.

### Prerequisite: hermetic test archetypes

Add tiny local archetypes under `crates/prova-cli/tests/fixtures/`:
- `arch-basic/` — one text prompt with a default, renders a couple of files. Baseline render proof.
- `arch-switched/` — a file gated on a `--switch`, to prove switch plumbing.
- `arch-undefaulted/` — a prompt with **no** default, to prove the `--headless` error path.

Point entry `source`s at these local paths so proofs are offline and fast.

### Proof inventory (the definition of done)

**`prova ide setup`**
1. In a package with a `prova.toml`, writes `.luarc.json` pointing at the core stubs; exit 0.
2. Idempotent — a second run leaves exactly one core entry.
3. `--manage never` installs stubs but writes no `.luarc.json`.
4. Core stubs land under the cache annotations dir, keyed by version.

**`prova init` catalog**
5. `--list` with no user config prints the built-in `default` (key + description).
6. `init default --headless` renders the default archetype into `.` (expected files present); exit 0.
7. Baked answer: config `[init.default].answers.proof_dir="tests"` → a `--headless` render uses it,
   observable in output; no prompt.
8. CLI `--answer proof_dir=x` **overrides** a baked config answer (precedence 1 > 2).
9. User config adds `service` → `--list` shows it and `init service --headless` renders it.
10. User config redefining `default` (different `source`/`description`) wins over the built-in.
11. `--switch feature` reaches the render (arch-switched emits the gated file only with it).
12. Clobber guard: `init default` in an already-initialized package exits non-zero; `--force` renders.
13. Unknown key: `init bogus` errors and lists the available keys; non-zero.
14. `--headless` with an undefaulted, unanswered prompt (arch-undefaulted) → clear error, no hang.
15. Non-TTY `init` with no key and >1 entry → clear error naming `--list` / `init <key>`; no hang.
16. After a successful `init`, IDE wiring ran (`.luarc.json` exists) unless `--no-ide`.

### Milestones (each is red → green)

- **M0 — fixtures.** The three `tests/fixtures/` archetypes. (Scaffolding for the proofs, not a proof.)
- **M1 — split IDE.** `ide.rs` + `prova ide setup`; `init` calls it at the end. Proofs 1–4, 16.
- **M2 — catalog model + `--list` + built-in default.** Proofs 5, 13.
- **M3 — non-interactive render dispatch** (`render_interactive`, `--answer/--switch/--defaults/
  --headless`). Proofs 6, 8, 11, 14.
- **M4 — config.toml load + merge + baked answers.** Proofs 7, 9, 10.
- **M5 — clobber guard + `--force`.** Proof 12.
- **M6 — interactive select** (inquire) + non-TTY fallback. Proof 15.
- **M7 — embed default archetype** (offline/hermetic) + docs + flag migration.

## Open questions / risks

- **archetect `use_defaults_all` semantics.** Confirm it means "use a prompt's default without
  prompting, but a prompt with no default still prompts" (interactive) and that `with_headless(true)`
  errors on any unanswerable prompt (CI). M3 pins this against `arch-undefaulted` before building on
  it.
- **inquire + non-TTY.** Verify `TerminalScriptIoHandle` fails cleanly (not a panic/hang) without a
  TTY; if it doesn't, prova detects `!stdin.is_terminal()` up front and refuses the interactive path.
- **Config precedence for `default`.** A user override of `default` fully replaces the built-in
  entry (no field-level merge) — simpler and predictable. Revisit only if partial override is asked
  for.
- **Binary size.** Embedding the default archetype adds a few KB of templates — negligible.
