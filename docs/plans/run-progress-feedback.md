# Run Progress & Status Feedback — turning silent pauses into understood latency

Drafted 2026-07-24 from a two-front audit of the reporting architecture (`model.rs`, `report.rs`,
`main.rs`, `mcp.rs`) and every blocking site on the run path (`modules.rs`, `engine.rs`,
`prova-archetect`, `plugins.rs`). Companion to the reporter design in
[architecture.md](../design/architecture.md) and the agent-facing contract in
[agent-ergonomics.md](../design/agent-ergonomics.md) (§5–§7: *misleading or absent output is a
first-class friction, and the MCP surface must not be a lossy subset*).

## The problem

A run can sit for tens of seconds with **zero output** and look hung. The dominant cause is a
Docker image pull (a fresh `postgres:16-alpine` is ~tens of MB over a cold registry), but it is
one of eight intrinsic pauses (table below). "Add a `print` in the plugin" cannot fix most of
them: they live *below* the plugin surface, inside prova's own kernel (`modules.rs`,
`engine.rs`), where no plugin author can reach.

## The frame: two concerns, not one

The word "progress" hides two different things, and the entire design is separating them:

1. **Status / activity** — *what is happening during this pause* (`pulling postgres:16-alpine…`,
   `waiting for postgres to accept connections…`). Cheap, safe, works in **every** output mode,
   and is exactly the context an agent needs to distinguish "provisioning" from "wedged." Plain
   lines on **stderr**.
2. **Live progress** — spinner, byte/layer bar, elapsed ticker. Transient, **TTY-only**, must
   never reach stdout or a captured pipe (where it degrades to escape-code soup).

> **Design axiom:** ship #1 *everywhere*; layer #2 on top *only* where stderr is a real terminal.
> The two have different lifetimes (ephemeral vs. durable) and different streams (stderr vs.
> stdout), so they are different mechanisms — not two settings of one.

## The architectural finding: this must NOT ride the `Event`/`Reporter` stream

The reporter seam (`crates/prova-core/src/model.rs:127`) carries exactly four events —
`RunStarted / NodeStarted / NodeFinished / RunFinished` — rendered to **stdout** (human tree,
JSON, TAP) or a file (JUnit), and in parallel runs marshalled across a thread channel as *owned*
data (`suite.rs:285`). It is a **test-lifecycle** stream. Every pause we care about happens
*inside* a node, in the module layer, which holds **no reporter handle**.

Adding a `NodeProgress`/`Activity` variant to `Event` would be wrong on three counts:

- It would emit onto **stdout**, corrupting `--format json` / `tap` and the JUnit document.
- It would force transient chatter through the owned-event channel that exists to marshal results.
- It would couple durable results to ephemeral status, which is precisely the seam
  [architecture.md](../design/architecture.md) keeps clean.

**Progress is a separate concern with its own stream (stderr) and its own lifetime.** It gets its
own thin abstraction, orthogonal to `Reporter`.

The good news is that stdout/stderr discipline is *already* established: every reporter writes
stdout; every `eprintln!` (warnings, `standing up topology…`, teardown errors at `engine.rs:610`)
goes stderr; color/TTY is delegated to `anstream::AutoStream`. A stderr-only progress facility
fits this grain exactly.

## The pause inventory (ranked by impact)

| # | Pause | Location | Today | Plugin-`print`-fixable? |
|---|---|---|---|---|
| 1 | **Docker image pull** | `modules.rs:3286-3301` | bollard `create_image` progress stream **drained and discarded** — per-layer progress is already in hand | **No — intrinsic** |
| 2 | `shell.run` (cargo/gradle/npm in fixtures) | `modules.rs:867` `.output()` | captured, silent until exit | Partly |
| 3 | `docker.build` (may itself pull base images) | `modules.rs:3002` `.output()` | captured, silent | No — intrinsic |
| 4 | Container readiness poll | `wait_ready` `modules.rs:3721-3792` | silent 30–60s worst case | No — intrinsic |
| 5 | `prova.retry` client readiness | `engine.rs:2341`, used at `modules.rs:718` | silent poll to timeout | No — intrinsic (shared) |
| 6 | Archetype render + first-time git clone | `prova-archetect/lib.rs:207-268` | join-blocks, serialized, silent clone | No — intrinsic |
| 7 | Plugin git fetch | `plugins.rs:638` | **prints _after_ the fetch** (`:643`) | No — intrinsic |
| 8 | `docker info` capability gate | `engine.rs:3610` (up to 8 backoff attempts) | silent before any test | No — intrinsic |

Two observations that shape the plan:

- **#1's data is already produced and thrown away.** `while let Some(item) = pull.next().await`
  drops each `CreateImageInfo` (layer id + status + progress detail). We do not need to parse
  `docker pull` output or add a dependency — we render what we already hold.
- **#7 is the pattern in miniature.** The `prova: fetching plugin {url}` line at `plugins.rs:643`
  fires *after* the slow `archetect_git_cache::resolve()` on `:638` returns — the message exists
  but lands once the wait is already over. Ordering, not absence, is its bug.

## The design: an activity side-channel, mirroring the reporter split

The repo's own division of labour — *prova-core stays unstyled; prova-cli owns the human
terminal* (`report.rs:1-9`) — dictates the shape:

- **prova-core** defines a minimal, presentation-free `trait Progress` and a `NullProgress`,
  carried as `Arc<dyn Progress + Send + Sync>` on `RunConfig` so both the sequential
  (`run_sequential`) and pooled (`run_pooled`) paths hold it; workers share the one `Arc`. The
  docker / shell / retry / render code calls it. **These call sites stabilize once.**

  ```rust
  // sketch — prova-core
  pub trait Progress: Send + Sync {
      fn begin(&self, id: TaskId, label: &str);       // "pulling postgres:16-alpine"
      fn update(&self, id: TaskId, detail: &str);     // "layer 3/7 · 22.4 MB"
      fn end(&self, id: TaskId, elapsed: Duration);   // completion (renderer decides what to show)
  }
  pub struct NullProgress;                              // default: does nothing
  ```

- **prova-cli** supplies the concrete renderer that writes to **stderr**, gated on
  `stderr.is_terminal()` + a `--progress` knob. The *presentation* evolves (plain lines →
  spinner → bar) without ever touching the core call sites.

This keeps results and activity fully orthogonal: `Event` → stdout (durable, machine-parseable);
`Progress` → stderr (ephemeral, human). It also makes `--format json` and the MCP JSON-RPC
channel *structurally* untouchable by progress — the safety is by construction, not by discipline.

### Threshold-gating (the "safe" ingredient)

Fast operations must stay silent, or the feedback becomes noise. The renderer records `begin`'s
timestamp and only *speaks* once an op crosses a threshold (~1.5–2s). The pull is the exception:
image-absent is known up front, so it announces immediately. Everything else (a warm `shell.run`,
a cache-hit render, an instant readiness check) produces nothing.

## How it fits every output mode (the safety matrix)

| Mode | stdout | Progress behaviour |
|---|---|---|
| `console` (TTY) | human tree | plain status line + optional single-line `\r` transient (spinner/bar) on **stderr**, cleared before completion |
| `console` (piped — e.g. an LLM capturing combined output) | human tree | plain bracketing lines only (`pulling…` / `pulled X (12.3s)`); **no `\r`, no escapes** — `is_terminal()` is false |
| `--format json` / `tap` | machine stream | stderr status lines only; never a byte on stdout |
| `--junit PATH` | file | unaffected |
| GHA (`--gha`) | `::error` etc. on stdout | stderr lines visible in the log; progress does not emit workflow commands |
| **MCP** | JSON-RPC (stderr invisible to client) | no spinner possible — separate levers, below |
| `--quiet` / `--progress=never` | as above | spinner suppressed / fully silent |

The **LLM payoff** is entirely in the plain-line path: a run that today shows nothing for 40s and
reads as "hung / timeout" instead reads `pulling postgres:16-alpine…` → `pulled (38.1s)`. That one
behavioural line converts a false-hang into understood latency. The `\r`/spinner path is what
would *hurt* an agent (escape soup in the transcript), so it is strictly `is_terminal()`-gated —
the same model `anstream` already applies to colour.

### MCP is a separate, smaller problem

`mcp.rs:934` is explicit: *over MCP stderr is invisible*, and the transport is request/response —
the `run` tool blocks, then returns one JSON blob (`mcp.rs:919`). No spinner applies. Two levers,
both deferrable:

- **(a)** MCP `notifications/progress` with a `progressToken` — worth a spike, but host support
  (including Claude Code) is uneven, so it cannot be the primary answer.
- **(b)** Make a slow call *explainable after the fact*: fold phase/elapsed into the returned
  result (per-node provisioning timing, or a `note`). This also closes agent-ergonomics §7(b)
  (`t:log` swallowed) territory — the result should carry what happened, not just the tally.

The CLI/TTY work is the main win; Phase-1 stderr lines already help whoever runs the MCP server in
a terminal.

## Parallel mode (`-j > 1`)

N suites pull concurrently, so a single spinner would lie. The reporter already made this call —
`GitHubReporter` deliberately omits `::group::` folding *"because parallel suites interleave"*
(`report.rs:216`). Match it:

- **`-j > 1`:** downgrade to plain **suite-prefixed** stderr lines, no cursor region.
- **`-j 1`:** the single-line `\r` transient is allowed.

Explicitly **avoid** an `indicatif` `MultiProgress` region: it fights the reporter's own
*unbuffered* stdout writes on a shared TTY (`report.rs:98-101`), which is the fiddliest possible
failure mode. Keep stdout (reporter) and stderr (progress) as two independent single-line concerns
that each clear before the other could collide.

## Knobs

Mirror the existing `--color` / `PROVA_COLOR` and `--gha` / `PROVA_GHA` triples exactly (CLI flag
> env > manifest key > `Auto`):

- `--progress=auto|always|never`, `PROVA_PROGRESS`, manifest `progress` key.
- `auto` (default): plain lines always; `\r` transient only when `stderr.is_terminal()`.
- `never`: fully silent (pristine CI logs).
- `always`: force plain lines even when piped (no `\r` — force does not fake a TTY).

## Phasing

**Phase 1 — safe, high-impact, no new deps, no cursor tricks.** Introduce `trait Progress` +
`Arc` on `RunConfig`; the cli renderer emits **plain, threshold-gated stderr lines**:

- Render the already-available pull stream at `modules.rs:3298` (announce on start; completion
  line with elapsed).
- Bracket `shell.run` / `docker.build` / `wait_ready` / `prova.retry` / render / git-fetch with
  threshold-gated start/done lines.
- Fix `plugins.rs:643` to speak **before** `resolve()`.
- Wire `--progress` / `PROVA_PROGRESS` / manifest key.
- Proofs: a run against an uncached image emits a `pulling` line to stderr and nothing to stdout
  under `--format json` (assert stdout stays valid JSONL); `--progress=never` is silent.

This alone kills the "hung" perception across CLI, piped, and LLM-capture, and cannot corrupt any
machine format.

**Phase 2 — TTY enrichment, renderer-only (no core changes).** Single-line `\r` spinner + elapsed
ticker for unbounded waits; a real aggregate byte/layer bar for the pull.
**Decision to make:** `indicatif` vs. a ~50-line hand-rolled stderr spinner. The single-static-
binary ethos and the hand-rolled `HumanReporter` lean hand-rolled; `crossterm` is already in the
tree transitively via `inquire`, so cursor control is available without a new top-level dep.

**Phase 3 — optional, only if a GUI/IDE frontend materializes.** A machine-consumable activity
stream (separate JSONL, or an activity event type consumed *only* by `JsonReporter`). Defer until
something needs it; don't over-build. (This is the one case where progress *could* legitimately
become structured — but on its own stream, never mixed into `Event`.)

## Decisions to record when this lands

- Progress is a **stderr-only side-channel** (`trait Progress` in core, terminal renderer in cli),
  **not** an `Event` variant. Fold this rule into [architecture.md](../design/architecture.md)
  next to the reporter seam.
- **Status ≠ progress:** plain lines everywhere; transient redraw is TTY-only and gated by
  `is_terminal()`, `--progress`, `--quiet`.
- Parallel runs use plain suite-prefixed lines, never a multi-progress region (consistent with the
  GHA reporter's no-`::group::` choice).

## Open questions

1. **Under `--quiet`, keep status?** Quiet suppresses PASS/SKIP but the "why is it paused" line is
   arguably the one thing worth keeping under quiet. Lean: keep the pull/readiness *start* line,
   drop the spinner. Confirm.
2. **MCP progress-notifications** — spike `rmcp` support and whether the host surfaces it, or go
   straight to result-side timing (lever b). Sequenced after Phase 1 regardless.
3. **`kind create cluster`** (`prova-kind`) pulls a large node image through `shell.run`'s
   `.output()` — it benefits from Phase 1's `shell.run` bracketing, but is there appetite for a
   richer, kind-aware line? Likely no; the generic bracket suffices.
