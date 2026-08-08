# Terminology consolidation — nailing down the lanes before 1.0

Triggered by the backlog work, which exposed that our vocabulary is "all over the place." Goal:
one predictable term per leg, a clean layer model, and the vestigial "spec = promise" usage
removed entirely (pre-1.0 is the reason to do it now, not defer it).

## The model

**Elevator pitch (whole system):** prova is an **executable agentic workflow** — an **agentic
state machine**. These two phrases are the one-floor description and the advanced-section framing.
They are NOT woven into per-leg learn/skill language; each leg has its own concrete term.

**Two layers, each a two-state duality, linked by binding:**

| layer | medium | cold/latent | active/owed | reconciled by |
|---|---|---|---|---|
| **spec** | prose (`docs/`) | `backlog` | `claim` | `owed` |
| **proof** | tests (`*.prova.lua`) | `promise` | `proof` | `attest` |

- **spec** is the *prose layer* — the medium, not a leg. A spec is the `docs/` surface that holds
  anchored obligations. Within it, an obligation is in one of two states: `backlog` (cold) ⇄
  `claim` (owed). "Two sides of the spec coin."
- **promise** = a commitment to make a spec real; **proof** = a demonstration that it is. `covers`
  binds a claim (spec layer) to a proof (proof layer).
- **reminder ⇄ heed** — the orthogonal attention rail.

Why this kills "spec = promise": a spec is prose (a claim), never a test. The old `spec/proof`
pairing clashed on tense because it named a *test* a *spec*. Fixed by giving "spec" its true
meaning — the prose layer.

## Predictability as a gate

Invariant: **for every verb `V`, `prova learn V` resolves (exits 0) and lands on `V`'s own
doctrine** (a dedicated topic, or its family topic via a principled command-keyword mapping — NOT
vanity synonyms). Canonical concept keys are **singular** (`spec`, `backlog`, `claim`, `promise`,
`proof`, `reminder`, `heed`); list-commands stay plural (`prova reminders`) but resolve to the
singular leg. Enforced by a proof (Increment 2), the mirror of the existing
`skill_and_topics_only_name_real_verbs`.

## Increments (each proof-carrying, committed separately)

1. **`[claims]` → `[specs]` config** *(done — this change)*. The section declares the prose that
   holds BOTH claims and backlog, so `[claims]` under-described it; `[specs]` is the truer name.
   `SpecsSection`, `manifest.specs`, key `docs` kept. All readers + 6 proofs + topic docs updated.
   Note: `Manifest` does not `deny_unknown_fields`, so a stale `[claims]` is silently ignored, not
   errored — external consumers (archetypes) must update their `prova.toml`. Follow-up candidate: a
   friendly "`[claims]` was renamed to `[specs]`" migration hint.
2. **`prova learn spec` + the learn-per-verb proof.** New `Topic::Spec` (key `spec`, `topics/spec.md`)
   describing the `[specs]` layer and its two anchor states. Repoint `spec`/`specs` learn aliases off
   promises onto the spec layer. Rename `Topic::Specs` → `Topic::Promises` (internal, key stays
   `promises`). Close every `prova learn <verb>` gap (`run`, `owed`, `attest`, `burndown`, `list`,
   `up`/`down`/…) via principled keyword mappings.

   **How the invariant is proven — a unit test, not black-box.** "Every verb resolves in learn" is a
   correspondence between two in-process source tables (`VERBS` ↔ `Topic::resolve`); the honest tool
   is a Rust unit test iterating them directly, sitting beside its existing siblings
   (`skill_and_topics_only_name_real_verbs`, `every_verb_documents_itself` — already unit tests, not
   `.prova.lua`). Black-box here would mean parsing `--help` to recover the verb list: fragile and
   indirect for no gain. This is the first clean exemplar of a broader direction (below): proofs are
   black-box OR unit tests, and prova is the single quality interface over both.

   **Direction — unit-test verdicts as first-class proofs (own design pass).** prova already ingests
   another verifier's verdicts via `junit`/`DeputedRow`. Extending that so prova adopts its OWN cargo
   unit-test junit into the account means `prova evidence`/`owed` speak for the whole quality surface
   — black-box proofs and typed unit invariants alike — and to a user it is just `prova`. Not required
   for the invariant above; tracked as its own exploration (candidate: `docs/design/verifiers.md`).

   *Finer grain candidate:* a proof whose body invokes ONE named unit test, with junit carrying the
   verdict — the same shape as adopting a whole junit file (`junit.load` + `DeputedRow`), narrowed to
   one selector. Makes "a claim discharged by a named unit test" identical to one discharged by a
   black-box proof, to `attest`/`owed`. Fallout: per-proof runner invocation is heavier than
   run-once/adopt-many (ergonomic ≠ implementation), and it couples that proof to the project's test
   toolchain — which is what deputing already is.
3. **Remove deprecated spec-as-promise surface.** The `prova specs` verb, `--specs`/`--strict-specs`
   flags, the legacy `{ spec = "reason" }` DSL attribute (`parse_spec_opt`), and their proofs/
   testdata (`testdata/spec*.lua`, `tests/spec.rs`, `promises_test`, `compatibility_test`). Only
   `promises = "..."` remains.
4. **`Outcome::Spec` → `Outcome::Promise` (wire rename).** The open-promise *outcome* is currently
   named "spec" — the deepest conflict. Rename `Outcome::Spec`/`Executed::Spec`/`summary.spec` and
   the MCP `spec` result field (currently "frozen" — unfreeze pre-1.0), bump record `schema` 1 → 2,
   and handle old records on read. Its own increment because it touches the record schema.
5. **Prose sweep.** Retire "the executable backlog" (= promises) → "the open spec surface"; introduce
   the umbrella ("executable agentic workflow" / "agentic state machine") in the pitch/positioning
   intros and advanced design sections only. Files: `skill.md`, `topics/{promises,running,pdd}.md`,
   `mcp.rs`, `docs/design/{positioning,burndown-lane,README,lifecycle}.md`.

## Open naming sub-decisions

- `[specs]` key: kept as `docs` (`[specs] docs = [...]`). Alternative considered: a top-level
  `specs = [...]` array. Easily flipped; kept `docs` for minimal churn.
- Whether `prova learn spec` is a full leg topic or an overview: shipping it as a **layer topic**
  (explains the medium + config, points to `learn backlog` / `learn claims` / `learn promises`).
