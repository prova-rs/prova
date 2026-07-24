# Positioning — why prova matters for agentic engineering

An assessment of where prova sits in the agentic-engineering landscape: what is genuinely
novel, what a skeptic will pattern-match it to, and what will decide whether it becomes
load-bearing infrastructure.

**Provenance, because it is itself evidence:** this assessment was written by an agent that
learned prova *exclusively* through the binary's own surface — `prova learn`, `introspect`,
and the MCP tools. No source files were read. That an agent can arrive cold, reconstruct the
entire model accurately, and evaluate its positioning is the autodidact thesis demonstrated,
not just claimed.

## The core thesis

Most agentic tooling makes agents **faster at producing claims**. Prova makes claims
**unnecessary**: it gives an agent a way to hand over evidence instead of assertions, and
gives the next agent a bar that survives the handoff. If agent-written code becomes the
majority case, something shaped like prova becomes load-bearing infrastructure — and nothing
else currently occupies the spot.

## What is genuinely new

In rough order of how hard each would be to copy:

### 1. The trust inversion

The deepest idea here is not testing — it is that in agentic engineering, **the verification
artifact is the durable thing and the agent is the ephemeral thing**. Context windows die;
the proof suite outlives them and re-imposes the bar on the next agent (or the next model).
The "proof-carrying change" is the right unit of work for a world where no human can review
every line an agent writes, but a human *can* review the contract the agent was held to.

The industry is mostly trying to solve agent trust with better review UIs. Prova solves it
with better evidence.

### 2. Specs — the executable backlog with built-in completion detection

The quietly brilliant piece. The spec flag gives agents what the field badly lacks: a
machine-legible work queue that cannot drift.

- `prova --specs --list` is a backlog that cannot lie (`git grep TODO` can).
- xfail-strict semantics: an open spec reports distinctly and keeps CI green; the moment a
  spec's body passes, it **fails** until the flag is deleted — so implementation and
  bookkeeping land atomically as one commit. There is no cleanup chore and no drift window.
- An agent can discover scoped, unclaimed work in a repo it has never seen, and knows —
  mechanically — when the work is done: the list is empty.

This is coordination infrastructure for multi-session, multi-agent development disguised as
a test annotation.

### 3. The closed loop: spec → proof → push → identical CI

The full lifecycle runs end to end with no translation step anywhere:

1. A contract is stated as a **spec** — executable, flagged, reasoned.
2. An agent drives it in the burndown loop (`--specs --strict-specs`), implements, and
   deletes the flag in the same commit — a proof-carrying change.
3. The change is pushed, and **the exact same proofs run in CI** via
   `prova-rs/run-action@v1` — same static binary, same suite, byte-identical to the local
   run. There is no "works locally, CI is configured differently" seam, because there is no
   second harness to configure.

Every prior methodology with this shape (BDD most famously) died in the translation gaps:
prose → step definitions → test code → CI config, each a place for drift. Prova has one
artifact and one binary at every stage, so the gaps do not exist.

### 4. CI as a work-executor: the deliberate burndown pipeline

The same mechanics invert what a pipeline is *for*. A conventional pipeline gates work a
human already did. A **burndown pipeline** runs `--specs --strict-specs`, hands the red
output to an agent, and lets it implement until the backlog shrinks — merging
proof-carrying changes as it goes.

This means the backlog is not just visible to agents; it is **executable by infrastructure**.
A team's role shifts to authoring specs (stating contracts) and reviewing proof-carrying
changes; the implementation lane between those two points can be scheduled compute. The
safety properties fall out of the existing semantics: an open spec cannot break the build, a
honored spec cannot land without its flag deleted, and an unflagged proof holds the line
immediately — so an autonomous lane is bounded by the same bar a human lane is.

No adjacent tool has this, because it requires the backlog, the verification, and the
completion signal to be the same artifact.

### 5. The autodidact surface

`learn` renders one screen per topic *for the current package* — proof locations, declared
plugins, topologies computed at call time. `introspect` serves the full API surface from the
same LuaCATS stubs that drive editor completion, so documentation cannot drift from what an
author sees. The MCP server ships its skill as `instructions`, so a connected agent starts
knowing the loop. Headless errors name the missing answer — the error message *is* the
interface. The design assumption — the reader is an agent with no context; never let it
guess — is one almost no tool has made cleanly.

### 6. Warm topologies over MCP

One environment definition addressed by every verb — fixture in tests, `up`/`start`/`watch`
in dev, held **warm inside the MCP server** for millisecond re-runs while iterating. This
attacks the actual bottleneck of agentic loops — iteration latency against real
infrastructure — rather than the perceived one (model speed). Dev, tests, and CI share one
definition, so they cannot drift.

## What a skeptic will say, and the honest answer

The individual ingredients have prior art, and skeptics will pattern-match to it:
Cucumber/Gherkin for "executable specs," Testcontainers for real-dependency tests, pytest
for fixtures and selection, Tilt/docker-compose for held environments.

The honest differentiator is not any ingredient but the **composition under one static
binary with an agent as the assumed operator**. None of the predecessors were designed to be
driven, learned, and held warm by an LLM; none made the backlog, the verification, and the
completion signal one artifact. Positioning should lead with the trust inversion and the
spec lifecycle — the parts with no real predecessor. "Black-box test runner with containers"
undersells prova into a crowded category it does not actually compete in.

## What will decide whether it is a big deal

- **Agent compliance out of the box.** The system rests on agents writing the proof first
  and never weakening assertions to get green. The spec-flag mechanic is
  enforcement-by-design; most of PDD is still enforcement-by-instruction (prompt-strength,
  not mechanism-strength). Every piece of doctrine that moves into mechanism — the way the
  spec flag did — compounds the value.
- **Plugin ecosystem gravity.** The facet grammar (`client` / `container` / `wait_for` /
  `mock`) means knowing one plugin is knowing all — but only if plugins exist for what
  people actually run. `docker.run` + `prova.containerized` is the escape hatch; breadth is
  the classic make-or-break for this shape of tool.
- **Inner-loop speed at real-repo scale.** PDD routes the definition of done through booted
  systems. If suite time creeps, humans route around it, and then agents do too. Warm
  topologies and surgical selection (`--last-failed`, tags, nodes) are the right answers;
  they must stay fast as suites grow.
- **A legible demo.** "Proof-driven development" is a category that has to be taught before
  it can be adopted. The dogfooding story — prova proven by prova, spec backlogs burned down
  by agents against the binary's own surface — is the most convincing artifact available.
  Making that loop visible to outsiders is worth as much as any feature.
