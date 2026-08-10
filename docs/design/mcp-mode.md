# MCP Mode & the Prova Skill — Prova as an agent-native proof tool

Drafted 2026-07-16. Builds on [topologies.md](topologies.md) (the held-environment substrate),
[proof-driven-development.md](proof-driven-development.md) (the workflow this serves), and the
selection work (`-k` / `--tags` / `--node` / `--last-failed`).

## Why

In Proof-Driven Development the agent's loop is: **write the proof → run it → implement → re-run
what's red → repeat**. Two frictions remain after selection shipped:

1. **Cold starts.** Every `prova` invocation re-provisions fixtures. Selection made *what runs*
   cheap; the containers and builds still dominate the loop (~30–60s per iteration on real suites).
2. **Cold agents.** An agent must be *taught* Prova per session. The tool should carry its own
   knowledge: connect (or `! prova skill`) and the agent knows Prova kung-fu — the idiom, the
   verbs, and which of its capabilities exist in the current transport.

MCP mode solves both with one move: **the MCP server is a topology holder with a tool surface that
mirrors the CLI.** Warm state comes from topologies (not a new state system); knowledge comes from
one embedded skill document served everywhere.

## The shape

```
prova mcp                 # stdio MCP server (rmcp, like archetect-mcp), resolved against the
                          # prova home exactly as the CLI is — same manifest, same packages
```

The server process owns a live engine (Lua state, package set, annotation-synced API). Held
topologies live **in the server's own scope machinery** — the same `ctx:manage` lifecycle as
`prova up`, with the server playing the role the attached `up` process plays today.

<!-- claim: held-visible-via-status-not-ps -->
Held topologies live in the server's **in-memory registry only** — `prova ps` lists *detached*
topologies (the `<home>/.prova/var/running/*.json` records from `prova start`); a server-held one is
visible through the MCP `status {}` tool, not `ps`.

### Tool surface — the CLI parity table

<!-- claim: mcp-cli-parity -->
The skill's contract: *"If Prova is an MCP server, call tools. If Prova is a CLI, run commands.
Everything else — the language, the grammar, the semantics — is identical."* That includes the
doors: `up` stands up a `[topologies]` registration on both transports, and nothing else.

| Capability | CLI | MCP tool | Notes |
|---|---|---|---|
| Run a selection | `prova -k … --tags … --node … --last-failed` | `run { keywords?, tags?, nodes?, last_failed?, profile?, jobs?, topology?, package? }` | Same `Selection` struct; returns one compact JSON summary `{ passed, failed, skipped, deselected, duration_ms, failures: [{ path, message, file?, line? }] }` |
| Discover | `prova --list` | `list { selection? }` | MCP returns nodes with path/tags/requires/file |
| One-shot code | `prova eval '<lua>'` *(new, ships with this work)* | `eval { code, topology? }` | Full environment (modules + packages). In MCP, `topology:` runs the snippet **inside a held env** — interactive queries against live seeded state |
| Hold an env | `prova up <name>` / `start` / `down` / `ps` | `up { name, fixed?, package? }` / `down { name }` / `status {}` | Server-held; endpoints in the result |
| API shape | `prova.help("<filter>")` in eval | `introspect { filter?, package? }` | `{ entries: [{ name, signature, summary }] }`, parsed from the LuaCATS stubs — core + declared packages |
| The topic catalog | `prova learn [<topic>]` | `learn { topic?, package? }` | Markdown, computed for the package at call time |
| **Warm re-run** | — (CLI runs are cold by design) | `run { …, topology: name }` | **The MCP-only capability**: tests resolve the named topology against the held instance — milliseconds, not provisioning |
| Failure detail | console/JSONL + `proc:output()` | failures carry attached output tails | Failure bundles ride the structured results |
| Know Prova | `prova skill` *(new)* | the server's `instructions` field | Same embedded document |

<!-- claim: selection-axes-parity -->
The table's "Same `Selection` struct" cell is enforced structurally, not by review. Every core
`Selection` axis is either exposed on the MCP selection surface under the same name or on an
explicit MCP-absent allowlist with its reason stated (the lane axes: a manifest/CLI concept the
raw MCP surface deliberately does not speak). A unit test walks the axes both directions —
core-axis-without-MCP-field is red, MCP-field-without-core-axis is red — and the axis list itself
is derived by exhaustively destructuring `Selection`, so adding an axis refuses to compile until
the test answers for it. `to_selection` was the one place the surfaces could drift silently
(`docs/plans/query-consolidation.md` invariant 2); a new axis now either reaches both surfaces or
names why not.

<!-- backlog: state-filters-from-lane-registry -->
**A lane's state filters should be generated from the lane registry, not hand-rolled per verb.**
Each lane verb currently parses its own state flags — `specs` hand-checks `--claims`/`--backlog`
mutual exclusion, `tests` hand-checks `--promises`/`--proofs`, `reminders` grew its pair last and
latest — and the MCP twin re-declares each as an argument. `prova_core::lanes::LANES` already
carries every lane's two state names; derive the flags from it (one optional `state` slot per lane
report, populated from the registry) and mutual exclusion becomes structural, a new lane cannot
ship without its filters on both surfaces, and alignment invariant 4 (state-filter parity,
`docs/plans/query-consolidation.md`) reduces to "the registry is consulted." Rides best with the
lane-polymorphic `Query { lane, selectors, state }` when the shared engine lands. Recorded
2026-08-09.

### Warm re-run: the one engine feature this needs

<!-- claim: warm-rerun-held-injection -->
Everything else is plumbing; this is the design's single piece of real engineering. Today
`t:use(env)` provisions the topology under the run's own scope. Warm mode needs **held-scope
injection**: a run whose `RunConfig` carries pre-instantiated topology values (the server's held
environment scope), so `t:use(env)` for a held name **resolves instead of provisions**, and the
run's scope-end teardown skips what it doesn't own.

This is the topology design's own separation paying off again: fixtures declare *ownership*
(`ctx:manage`), scopes decide *when* — warm injection just adds *whose scope*. Ownership rule:
**the holder tears down; the run never reaps injected instances.** Consistency caveat, stated
honestly in the skill: a warm environment accumulates state across runs (that's the point); the
agent resets by `down`/`up` when isolation matters, exactly like a developer would.

### `prova eval`

<!-- claim: eval-one-shot -->
CLI: `prova eval 'return archetect.render{...}.path'` — collect nothing, run the snippet in a
scratch test context (fixtures available via `require`/globals; `ctx`-style helpers exposed as a
transient scope), print the returned value (human) or JSON (`--format json`). Kills the
probe-file ceremony an agent otherwise performs. MCP `eval` is the same execution path; with
`topology:` it evaluates inside the held env's state, e.g.
`eval { code = "return orders.db.client:query('select * from orders')", topology = "orders" }`.

## The Prova Skill

<!-- claim: skill-embedded-everywhere -->
**One document, embedded in the binary** (`include_str!` — versioned with the features it
describes, so it can never drift), delivered three ways:

1. `prova skill` — prints to stdout. An agent session ingests it with `! prova skill`.
2. **MCP `instructions`** — served on connect; MCP agents "just know" without any command.
3. `prova skill --install` — writes `.claude/skills/prova/SKILL.md` so repos carry it durably.

Structure (universal-first, transport notes last — avoiding duplicated skills):

- **What Prova is for you (the agent):** write proofs, not just tests — executable black-box
  definitions of done; lean on Prova for verification instead of claiming success. The PDD loop.
- **The idiom, compressed:** fixtures + scopes, the resource grammar (`{ client, url, container,
  host, port }`), packages (`[dependencies]` + `require`), topologies (one definition, test/up/eval all
  consume it), quiet primitives (`check = true`, scalar env, `proc:output()`), selection
  (`-k`/`--tags`/`--node`/`--last-failed`), snapshots, the variant-loop pattern for matrices.
- **The loop:** scaffold with `prova init`; probe with `eval`; write the proof; run; implement;
  `--last-failed` until green; hold a topology when iterating against live infra.
- **Driving Prova (the only transport-specific section):** the parity table above, ~15 lines.

<!-- claim: project-card-self-teaching -->
**`prova learn project` is the project card: an agent must be able to "just know" how THIS
package runs, with no CLAUDE.md prose to maintain.** The card names, computed at call time: where
prova's own artifacts live (the manifest variant, the `config` companion, the `.prova/var/` state
dir); where specs are stored and where a new one should be written (the `[[specs.source]]` roots,
each marked writable); where proofs go (the `[run] proofs` patterns); the declared capability
surface and how to check the host (`prova capabilities`); every profile with its description,
selection, thrown switches, and guarantees — so "which profile, when?" is answered by the tool;
and the declared switches with who throws them (`prova switches` for the live inventory). This is
the adoption thesis: a novice with an agent loads prova and gets spec-driven engineering as a
matter of course, because the binary teaches its own practice — prose files drift, computed cards
cannot.

<!-- backlog: backlog-capture-is-a-taught-procedure -->
**"Add a backlog item" must be a taught procedure, not a syntax an agent reassembles.** The skill
and `prova learn backlog` teach the anchor's *shape* — the keyword, promotion, the draw-down date —
and neither teaches *where it goes*, so an agent told to capture something guesses at a file, and a
plausible guess (a plan, a README, a scratch note) lands the item somewhere the ledger never scans:
capture that silently does not capture. Three steps, taught as one: **(1)** ask the config which
spec sources exist and which of them can be written to
(`docs/design/manifest.md#spec-sources-are-queryable`); **(2)** locate the spec file whose subject
the item belongs to, or create one under a writable source; **(3)** write the anchor with the date
it was captured (`docs/design/lifecycle.md#anchor-records-when-it-was-captured` — until that lands,
the capture date goes in the item's prose, because the anchor's one date slot means *deadline*).
The same three steps are what an MCP `backlog` tool would have to perform, so the procedure is the
tool's spec as much as the skill's text. Recorded 2026-08-08.

## Phasing

1. **`prova skill` + `prova eval`** — pure CLI, immediate agent value, no MCP dependency. The
   skill document is also the forcing function to write Prova's knowledge down once, well.
2. **`prova mcp` cold** — rmcp stdio server: `run`, `list`, `eval`, `skill`-as-instructions,
   structured results. Already better than shelling for hosts without a shell.
3. **Warm** — held-scope injection in the engine; `up`/`down`/`status` tools; `run{topology}`;
   `eval{topology}`. The headline.
4. **Failure bundles** — attach managed proc/container output tails to failed-node results (both
   transports; designed separately, lands naturally here).

## Non-goals

- No session/state system separate from topologies — one holder concept, one teardown path.
- No MCP-only capabilities beyond warmth: anything the server can do cold, the CLI can do, so the
  skill stays "everything else is identical."
- No skill duplication per transport: one document, one conditional section.
