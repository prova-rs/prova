# API Freeze — decisions before the spec suite

Ratified 2026-07-23. These are the coherence/breaking decisions frozen *before* authoring the
comprehensive spec-as-proofs suite (see [intrinsics-gap-assessment.md](intrinsics-gap-assessment.md)
for the gap inventory this serves). Everything below is normative: the spec suite encodes it.

## 1. Format namespaces — BREAKING, clean cut

Technology-first sibling modules, encode + decode together; `prova.parse.json` is **removed**
(no alias — pre-1.0, break cleanly):

- `json.decode(s)` / `json.encode(v, opts?)` — plus fidelity sentinels: decode keeps `null → nil`;
  encode and subset shapes accept **`json.null`** (assert/emit explicit null) and **`json.array{}`**
  (force `[]` for empty/ambiguous tables; bare `{}` encodes as `{}` object).
- `yaml.parse(s)` / `yaml.parse_all(s)` / `yaml.dump(v)` / `yaml.dump_all(docs)` — multi-doc emit
  for k8s-shaped streams. Same sentinels as json.
- `toml.parse(s)` / `toml.encode(v)` — dep already in-tree.
- `csv.parse(s, opts?)` / `csv.encode(rows, opts?)` — header-aware; row shape mirrors
  `prova.parse.table`.
- `xml` — **deferred** until real demand; heavy to do well.
- `prova.parse.{lines,rows,table}` stay — format-agnostic text utilities, correctly homed.

Utility belt (separate from formats, same grammar, all reserved): `base64.{encode,decode}`,
`hash.{sha256,hmac_sha256}`, `uuid.v4()`, `url.{parse,encode}` (dep in-tree, unexposed today).

## 2. Globals, collisions, and reserved names — the injection contract

Three mechanisms close every silent-collision path:

1. **Reserved-name registry.** All bundled namespace names (`prova`, `Scope`, `shell`, `fs`,
   `net`, `http`, `docker`, `sqlite`, `grpc`, `graphql`, `json`, `yaml`, `toml`, `csv`,
   `base64`, `hash`, `uuid`, `url`, + future kernel transports `socket`, `terminal`) are
   reserved. A `[plugins]` entry or plugin-root file bearing a reserved name is a **manifest
   validation error** — never a silent shadow in either direction.
2. **Write-protected globals.** A `_G` metatable makes *assignment* to a reserved name raise:
   `cannot assign to 'fs' — it is a prova namespace; use a local, or exclude it in [run]
   globals`. Local shadowing (`local fs = ...`) remains legal and untouched — lexical, visible,
   deliberate.
3. **Configurable injection over `require`.** Every bundled namespace is `require`-able by name
   (the searcher's bundled tier — already the dogfooding direction). Global injection is sugar:
   `[run] globals = { exclude = ["fs"] }` removes a name from injection; the team then does
   `local fs = require("fs")` (any local name) where wanted. Default remains all-injected —
   batteries included, collisions loud or configured, nothing silent.

## 3. Structural subset matching — `:matches`, one semantics, three surfaces

Polymorphic by argument (precedent: `contains`):

- string arg → Lua pattern (unchanged today).
- table arg → **recursive structural subset**: every key present in the shape must exist in the
  subject and recursively match; extra subject keys ignored; arrays match same-index recursive
  (shape `[i]` vs subject `[i]`), shape shorter than subject is fine, longer fails; `json.null`
  in a shape asserts the subject holds explicit null (decoded as… absent-vs-null pinned in the
  spec); scalar leaves compare with `values_equal` (int/float coercion as today).

The **same matcher** backs all three surfaces — `t:expect(x):matches{...}`, `double:on{...}`
(replacing double.lua's private Lua implementation, whose semantics seed the spec), and mock
stub matchers (`m:on{ path=..., body={...}, headers={...}, query={...} }`). Failure output uses
the new **table-aware path diff** (`status.readyReplicas: expected 3, got 1`), replacing
`display()`'s `<table>` collapse everywhere tables are rendered.

## 4. `:eventually` — the poll-until modifier

`t:expect(fn):eventually(opts?):<matcher>` — legal only when the subject is a function;
re-evaluates `fn` and the terminal matcher until pass or timeout (`opts = { timeout, every }`,
defaults shared with `prova.retry`, which remains the underlying primitive and stays public).
On timeout the failure renders the **last** value with the path diff. Non-function subject +
`:eventually` = clear error.

## 5. `spec` — proofs authored ahead of implementation

Named `spec`, not "pending": in PDD vocabulary a proof not yet honored *is* the specification —
"pending" describes a state, `spec` names what the thing is. Semantics are xfail-strict with
**per-test inversion**, which is what removes any after-the-fact cleanup chore or drift window.

> **REVISED 2026-07-23 after first dogfooding**: `spec` is **test-level only** — no group,
> suite, or `suite.lua` flags, and no `spec = false`. The original draft had inherited flags
> plus graduation markers, and every awkward piece of the lifecycle (markers on finished
> proofs, the orphan-marker error, the completion error, remedy wording that depended on where
> the flag lived) existed solely to service inheritance. A test is either flagged as a spec or
> it is a full proof with nothing to indicate. Per-test flags also carry per-test reasons —
> better documentation than one blanket reason; bulk-authoring is what agents are for.

- **Where set:** on a `test` or `flow` only — `{ spec = true }` or `{ spec = "reason/ticket" }`.
  A `spec` on a group or in `suite.config` is a validation error naming the fix; `spec = false`
  is not a thing (an unflagged test is already a full proof).
- **Open spec** (spec'd test that fails) → distinct `spec` outcome in every reporter (TAP: the
  `# TODO` directive — exactly these semantics; JUnit: skipped + message; JSONL: outcome
  `"spec"`; console: reason + first error line, no traceback). CI stays green.
- **Spec that passes → failure**: "spec honored — remove the spec flag from this test." An
  implementation cannot land without deleting its flag in the same commit — cleanup is forced
  at implementation time, never proactive-after-the-fact. Implementation + flag removal = a
  proof-carrying change, and the finished proof carries no annotation.
- **No mid-burndown drift window**: an unflagged test holds the line immediately; open specs
  are red by definition — no state exists where a regression can hide.
- `prova --specs` — a **selector** (like `--last-failed`): run exactly the tests currently
  carrying a spec flag — red report as open specs, green fail demanding flag removal.
  Composes: `--specs --list` enumerates the remaining surface without running; the run summary
  counts `N spec open`.
- `prova --strict-specs` — driver mode: open specs are real failures (full detail, traceback
  included). The implementing agent's inner loop is `--specs --strict-specs`; CI runs neither.
- This feature is **implemented first, spec'd by hand** — everything else's spec depends on it.

## 6. Journal standardization — one `received()` vocabulary

All observation journals (`http.mock`, `grpc.mock`, `prova.double`, future transports) share:
`seq` (monotonic per mock), `source` (`stub|target|unmatched|…`), `matched` (bool), plus
transport-native payload fields (http keeps `method/path/query/headers/body/params/status`;
grpc keeps `method/request/code`). Filters accept the same subset-matcher shapes as `:on`.

## 7. Vocabulary lines (frozen, not breaking)

- `delay` = per-reply one-shot (shipped) · `latency/drop/corrupt/throttle/after` = continuous
  proxy fault verbs (future). Both words, distinct meanings.
- Driver observation: `wait_for` (readiness) / `expect` (observe-until-match, timeout'd) —
  future `Process:expect(pattern)` and `terminal:expect` conform.
- `ctx:log` is promoted to a real **Log event** in the report stream (today stderr-only).
- Naming: **Mock** (transport seam) / **double** (function seam) / never "double mocks" — with
  the Meszaros glossary folded into mocks-proxies-drivers.md.

## Execution: the spec-as-proofs experiment

The concept under test: **the entire remaining API is spec'd as open-spec proofs, then
implemented systematically** (agent-driven `--specs --strict-specs` loop, each landed feature
graduating its spec in the same commit — proof-carrying changes throughout).

Order:
1. The `spec` engine feature + its hand-written selftest (the bootstrap). **DONE.**
2. Freeze items as spec suites, roughly one directory per capability: `proofs/spec/formats/`
   (json/yaml/toml/csv round-trips, sentinels), `proofs/spec/matching/` (subset semantics
   table — the largest and most valuable single suite, `test_each`-driven), `proofs/spec/
   eventually/`, `proofs/spec/globals/` (reserved names, write protection, exclusion),
   `proofs/spec/journals/`, then the Tier-A transports as they are designed
   (stub matchers, faults, TLS, streaming, socket, terminal).
3. Implementation burndown against `--specs --strict-specs`, trust-track hardening interleaved
   per the gap assessment's sequence.

**Burndown status (2026-07-24):** §1 formats (`json`/`yaml.dump`/`toml`/`csv`) + utility belt,
§3 matching (incl. the `json.null` sentinel), and §4 `:eventually` are **implemented and
graduated** — their suites run flag-free, `prova --specs --list` is empty. `prova.parse.json` is
removed and callers migrated. Still to spec-then-implement: §2 globals
(`proofs/spec/globals/` — reserved names, write protection, require-injection), §6 journals
(`proofs/spec/journals/`), and the Tier-A transports as they are designed.

**Spec-engine ergonomics (2026-07-24, spec'd in `proofs/spec/engine/`):** the flag combos are
the composable primitives but a poor entry point (`--strict-specs` is the thing you reach for
most and the least memorable spelling). DECISION: the lifecycle gets **verbs**, matching the
grammar where activities are subcommands and no-arg subcommands list their domain (`prova up`,
`prova plugins`) — `prova specs` enumerates the open surface, `prova burndown` is the inner
loop (spec-selected, open specs fail loud), subsuming `--specs --strict-specs`. `--specs`
survives as the selector that composes with path selection. `--specs --list` now carries its
own guardrail proof (the engine was bootstrapped "implemented first, spec'd by hand"; that gap
is closed).
