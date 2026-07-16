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
                          # prova home exactly as the CLI is — same manifest, same plugins
```

The server process owns a live engine (Lua state, plugin set, annotation-synced API). Held
topologies live **in the server's own scope machinery** — the same `ctx:manage` lifecycle as
`prova up`, with the server playing the role the attached `up` process plays today. `prova ps`
lists server-held topologies alongside detached ones (same `<home>/running/*.json` records,
tagged with the holder).

### Tool surface — the CLI parity table

The skill's contract: *"If Prova is an MCP server, call tools. If Prova is a CLI, run commands.
Everything else — the language, the grammar, the semantics — is identical."*

| Capability | CLI | MCP tool | Notes |
|---|---|---|---|
| Run a selection | `prova -k … --tags … --node … --last-failed` | `run { keywords?, tags?, nodes?, last_failed?, profile? }` | Same `Selection` struct; MCP returns structured events (the JSONL shapes), not text |
| Discover | `prova --list` | `list { selection? }` | MCP returns nodes with path/tags/requires/file |
| One-shot code | `prova eval '<lua>'` *(new, ships with this work)* | `eval { code, topology? }` | Full environment (modules + plugins). In MCP, `topology:` runs the snippet **inside a held env** — interactive queries against live seeded state |
| Hold an env | `prova up <name>` / `start` / `down` / `ps` | `up { name, fixed_ports? }` / `down { name }` / `status {}` | Server-held; endpoints in the result |
| **Warm re-run** | — (CLI runs are cold by design) | `run { …, topology: name }` | **The MCP-only capability**: tests resolve the named topology against the held instance — milliseconds, not provisioning |
| Failure detail | console/JSONL + `proc:output()` | failures carry attached output tails | Failure bundles ride the structured results |
| Know Prova | `prova skill` *(new)* | the server's `instructions` field | Same embedded document |

### Warm re-run: the one engine feature this needs

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

CLI: `prova eval 'return archetect.render{...}.path'` — collect nothing, run the snippet in a
scratch test context (fixtures available via `require`/globals; `ctx`-style helpers exposed as a
transient scope), print the returned value (human) or JSON (`--format json`). Kills the
probe-file ceremony an agent otherwise performs. MCP `eval` is the same execution path; with
`topology:` it evaluates inside the held env's state, e.g.
`eval { code = "return orders.db.client:query('select * from orders')", topology = "orders" }`.

## The Prova Skill

**One document, embedded in the binary** (`include_str!` — versioned with the features it
describes, so it can never drift), delivered three ways:

1. `prova skill` — prints to stdout. An agent session ingests it with `! prova skill`.
2. **MCP `instructions`** — served on connect; MCP agents "just know" without any command.
3. `prova init --skill` — writes `.claude/skills/prova/SKILL.md` so repos carry it durably.

Structure (universal-first, transport notes last — avoiding duplicated skills):

- **What Prova is for you (the agent):** write proofs, not just tests — executable black-box
  definitions of done; lean on Prova for verification instead of claiming success. The PDD loop.
- **The idiom, compressed:** fixtures + scopes, the resource grammar (`{ client, url, container,
  host, port }`), plugins (`[plugins]` + `require`), topologies (one definition, test/up/eval all
  consume it), quiet primitives (`check = true`, scalar env, `proc:output()`), selection
  (`-k`/`--tags`/`--node`/`--last-failed`), snapshots, the variant-loop pattern for matrices.
- **The loop:** scaffold with `prova init`; probe with `eval`; write the proof; run; implement;
  `--last-failed` until green; hold a topology when iterating against live infra.
- **Driving Prova (the only transport-specific section):** the parity table above, ~15 lines.

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
