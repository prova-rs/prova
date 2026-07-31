# Proposal: platform-agnostic APIs (so proofs "just run everywhere")

**Status: proposal for review, 2026-07-28. Not implemented.**

## Why

Widening the black-box proof gate to Windows surfaced 64 failures. ~60 share one root cause:
proofs reach for POSIX shell idioms (`shell.run("mkdir -p …")`, `2>&1`, `printf … >`, `|`) that on
Windows execute through `cmd /C`, where `-p` isn't a flag, `/` isn't a separator, and redirects
differ. The reflex fix — "run everything under bash on Windows" — is **antagonistic** to prova's
premise of *something that just runs everywhere*: it trades a real cross-platform runtime for a
dependency on a POSIX shell being installed, and it leaves the proofs written in a dialect that only
one shell speaks.

The durable fix is the opposite: **grow native, platform-agnostic APIs so a proof never has to shell
out (or reinvent a utility) for a common operation.** Legitimate `requires = { "windows" }` / unix
gating stays fine for genuinely platform-specific things — but we should not *manufacture* the need
for it by lacking a portable API. Every gap we close removes a reason to fork a proof per platform.

This proposal reviews the current surface, proposes the missing pieces (filesystem + path + string/
casing), and analyses growth, stability, and collision with existing/user-defined symbols.

## Two design principles first

**1. Forward slashes, everywhere.** A large share of the Windows failures are not "missing API" —
they're *backslashes*: `fs.tempdir()` returns `C:\Users\RUNNER~1\…`, and that value then breaks a
TOML string (`p = "C:\Users…"` → invalid `\U` escape — the same class as the catalog bug), a shell
command, and a path-pattern assertion. Rust's `std` and git-bash both accept `/` on Windows. So
prova should **emit `/`-normalized paths from every path-producing API** (`fs.tempdir`,
`ctx:tempdir`, `fs.glob`, `path.*`) and accept either separator on input. This dissolves the whole
backslash class at the source. (One behavior change: assertions expecting native separators — there
is exactly one, asserting a `\\?\D:\…` path against a unix pattern; forward-slash *fixes* it.)

**2. The argv form is the portable form; give it what only strings had.** `shell.run(argv_table)`
already runs the program directly with no shell — the portable path. Proofs drop to the *string*
form (a shell) only for three shell features: stream redirect (`2>&1`), stdin piping (`printf x |
cmd`), and sequencing (`a && b`). Add the first two as options and the argv form covers almost every
current shell-string use portably; sequencing is better expressed as two calls or an `fs` op.

## Current surface (for reference)

Bundled global namespaces (all in `RESERVED_NAMESPACES`, `lib.rs`): `prova`, `Scope`, `shell`, `fs`,
`net`, `http`, `docker`, `sqlite`, `grpc`, `graphql`, `json`, `yaml`, `toml`, `csv`, `base64`,
`hash`, `uuid`, `url`, `socket`, `terminal`, `websocket`. Plus the `archetect` **plugin** namespace.

`fs` today is six ops: `exists`, `read`, `write` (creates parents), `remove_all`, `tempdir`, `glob`.
There is **no** `path`, `str`/`strings`, or casing namespace anywhere.

## Proposed additions

### A. Extend `fs` (non-breaking; no new reservation needed)

By proof pressure, in order:

| Add | Retires | Notes |
|---|---|---|
| `fs.mkdir(path)` | **20 `mkdir -p` sites** | Creates all parents, idempotent (no error if present). `std::fs::create_dir_all`. The single highest-value gap — `fs.write` already covers the *file* case, so the residue is all "an empty dir must exist." |
| `fs.remove(path)` | some `rm` | Single file/dir remove; complements `remove_all`. |
| `fs.read_dir(path)` → `string[]` | `ls` | Non-recursive listing (glob is recursive-pattern). `/`-normalized. |
| `fs.copy(src, dst)`, `fs.move(src, dst)` | latent | No sites yet, but absent from the surface; cheap and expected. |
| `fs.stat(path)` → `{ size, is_file, is_dir, mtime }` | version/size checks | Metadata without shelling to `stat`. |
| `fs.is_file(path)` / `fs.is_dir(path)` | — | Predicate *functions* (today only via `expect` matchers). |
| `fs.tempfile()` → path | — | Complements `tempdir`. |
| `fs.make_executable(path)` | 2 `chmod +x` | Sets the unix exec bit; **no-op on Windows** (executability is by extension there). Lower urgency — the 2 sites are unix-gated shim tests. Prefer this over a raw `fs.chmod(path, mode)` because a numeric mode is a unix concept that doesn't port. |

### B. New `path` namespace (reserve `path`)

Pure, platform-agnostic path algebra — no filesystem access, returns `/`-normalized strings.

- `path.join(a, b, …)` — the OS-correct join, always emitting `/`.
- `path.dirname(p)`, `path.basename(p)`, `path.ext(p)`, `path.stem(p)`.
- `path.normalize(p)` — collapse `.`/`..`/duplicate separators, strip trailing slash.
- `path.is_absolute(p)`.

Retires pervasive `dir.."/proofs/a_test.lua"` concatenation and the ad-hoc `dir:match("([^/\\]+)$")`
basename. This is the most *pervasive* Windows-hostility (hardcoded separators everywhere) even
though it isn't the loudest failure.

### C. New `str` namespace (reserve `str`) — general string utils **+ casing from archetect**

One discoverable home for string work, including the casing vocabulary sourced from archetect so we
get **unity of function and naming** across the two tools.

**General utilities** (retire reinvented boilerplate):
- `str.trim(s)`, `str.trim_start(s)`, `str.trim_end(s)` — retires **8 `gsub("%s+$","")` sites**.
- `str.split(s, sep)`, `str.lines(s)` — unify with the under-discovered `prova.parse.lines`.
- `str.starts_with`, `str.ends_with`, `str.contains`.

**Casing / inflection — call `archetect_inflections::*` directly** (do NOT reimplement). Function
names mirror archetect's template-filter names verbatim, so a name learned in an archetype template
is the same name in a prova proof:

| `str.*` | archetect `archetect_inflections::` fn | `"my example string"` → |
|---|---|---|
| `snake_case` | `to_snake_case` | `my_example_string` |
| `pascal_case` | `to_pascal_case` | `MyExampleString` |
| `camel_case` | `to_camel_case` | `myExampleString` |
| `kebab_case` | `to_kebab_case` | `my-example-string` |
| `train_case` | `to_train_case` | `My-Example-String` |
| `constant_case` | `to_screaming_snake_case` | `MY_EXAMPLE_STRING` |
| `cobol_case` | `to_cobol_case` | `MY-EXAMPLE-STRING` |
| `title_case` | `to_title_case` | `My Example String` |
| `sentence_case` | `to_sentence_case` | `My example string` |
| `class_case` | `to_class_case` | `MyExampleString` (Pascal, last word singularized) |
| `package_case` | `to_package_case` | `my.example.string` |
| `directory_case` | `to_directory_case` | `my/example/string` |
| `pluralize` | `to_plural` | `crate` → `crates` (dictionary-smart) |
| `singularize` | `to_singular` | `replies` → `reply` |
| `ordinalize` | `ordinalize` | `1` → `1st` |

Plus the `is_*` predicates (`str.is_snake_case`, …) which archetect ships but doesn't currently
expose. `lower`/`upper` are plain `to_lowercase`/`to_uppercase` (not in the crate). This is a
*latent* gap — no proof converts case today — but it is the strategically central one: prova's job
includes proving archetect archetypes, whose whole value is rich casing, and suites will grow their
own helpers the moment they start asserting on cased output. Ship it before that divergence starts.

### D. Extend `shell.run` opts (no new namespace)

- `shell.run(argv, { merge_stderr = true })` — fold stderr into stdout. Retires **35 `2>&1` sites**,
  moving them from shell-string to portable argv form.
- `shell.run(argv, { stdin = "…" })` — feed input without a `printf … |` pipe.

Sequencing (`&&`, `;`) is intentionally *not* added — express it as multiple `shell.run` calls or an
`fs` op (`mkdir && printf > f` becomes `fs.mkdir` + `fs.write`).

### E. Candidates (lower priority, note-only)

- `semver.parse(s)` / `semver.satisfies(v, req)` — retires repeated `("(%d+%.%d+%.%d+)")` matching.
- `env` helper with an **unset** spelling (`shell.run`'s `env` only extends; there's no way to unset).
- Discoverability, not API: `json.encode` is reinvented in 2 selftests; `prova.parse.lines` is
  under-adopted. A `learn` topic / lint nudge, not new surface.

## Adopted namespace model: one canonical `prova.*` + declared injection

**Decided 2026-07-28. Breaking (existing suites migrate) — acceptable pre-freeze.** This supersedes the
top-level-global model and unifies four concerns into one mechanism.

**Canonical: `prova.*` is prova's first-party surface.** Every bundled module is reachable as
`prova.<name>` (`prova.fs`, `prova.http`, `prova.str`, `prova.path`, `prova.grpc`). `prova` is the
*only* guaranteed global, and `prova.fs` always resolves — injected or not — so shared helpers have a
stable, unambiguous spelling. **Plugins do NOT join `prova.*`** (no `prova.postgres`): a third party
does not share prova's own namespace — it would blur ownership and risk colliding with a future
first-party name. Plugins are reached as `require("postgres")` (as always) and *optionally* bound as a
bare unqualified global via injection (below). (Mechanically: the existing `prova.namespaces` registry
becomes the canonical `prova.*` surface for bundled modules; plugins stay in the searcher's registry
tier for `require`.)

**Injection is declared, per package.** A `[globals]` section lists which modules are *also* bound as
unqualified ambient globals — the DSL sugar:
```toml
[globals]
inject = ["fs", "http", "shell", "expect", "postgres"]   # bundled AND plugin names, uniformly
```
Only injected names are write-protected (assignment raises); every non-injected name is entirely the
user's (`local fs = flight_simulator()`, or even a global `fs`, is fine). This **replaces** the old
opt-out `[run] globals.exclude` with an explicit opt-in list.

**Uniform participation — first- and third-party inject identically.** `inject = ["fs", …, "postgres"]`
binds `postgres.*` unqualified next to `fs.*` by the same one line. (Injecting a plugin loads it
eagerly at engine setup rather than lazily via `require`.) The old question "how do I get `fs`
unqualified" vs "how do I get `postgres` unqualified" now has one answer.

**Defaults + no mystery.** When `[globals]` is absent, a sensible default inject set applies (the
common core) so a bare manifest / `prova eval` still reads unqualified. But the **archetype-generated
`prova.toml` writes the inject list explicitly and in full**, so a real project *shows* its globals
rather than inheriting invisible ones. When present, the list is authoritative (what you see is what
you get — not "defaults plus these"). Profiles override it wholesale, matching today's semantics.

**Growth is collision-free.** A new bundled module is just a new field on `prova` (`prova.str`,
`prova.path`) — it claims **no** ambient global and reserves **no** top-level name until a package
chooses to inject it. So `str`/`path` — the highest-collision names — never squat a user's global by
default. `RESERVED_NAMESPACES` keeps its narrower job: the set of *known first-party module names*,
used to populate `prova.*` and to reject a plugin that tries to claim one — not "the ambient set."

**Stability / migration.** Additive at the module level (new `prova.*` entries), breaking at the
manifest level (`inject` replaces `exclude`) and for any suite that relied on a now-non-default
ambient global — those migrate to injecting the name or writing `prova.<name>`. The one behavioral
change beyond naming is principle #1 (path outputs normalize to `/`).

**Core-vs-plugin for casing.** Casing is generically useful, not archetect-specific, so `str` lives in
**core** — a proof shouldn't need the render plugin loaded to snake-case a string. The cost:
prova-core gains a dependency on `archetect-inflections`, a small stable leaf inflector fork with no
heavy transitive deps. prova already pins the archetect workspace, so the coupling is low-risk and the
naming/behavior unity is the whole point.

## Payoff: this is the path to the Windows gate

Concretely, closing the Windows proof gate ≈ this proposal applied:
- `fs.mkdir` + migrate ~19 sites → clears the dominant failure class.
- `shell.run { merge_stderr, stdin }` + migrate the 35 `2>&1`/pipe sites → clears the shell-feature
  class.
- `/`-normalized paths → clears the TOML-escape and path-pattern failures.
- `path.*` → clears the hardcoded-separator hostility that would otherwise reappear.

Then the gate widens to Windows for real — one suite, three platforms, no fork.

## Suggested sequencing

1. Reserve `path` and `str` in `RESERVED_NAMESPACES` (cheap, claims the names).
2. `fs.mkdir` + `/`-normalized path outputs (highest failure-clearing leverage).
3. `shell.run { merge_stderr, stdin }`.
4. `path.*`.
5. `str` general utils (`trim`/`split`/…), then `str` casing from `archetect_inflections`.
6. Migrate proofs off shell-outs; widen the Windows proof gate.
7. Nice-to-haves: `fs.copy/move/read_dir/stat/tempfile/make_executable`, `semver`, `env` unset.
