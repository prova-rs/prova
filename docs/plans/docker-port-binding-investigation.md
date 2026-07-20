# Docker port-binding investigation — handoff

**Status:** one bug found and fixed; the original case is still open. The instrument that would
settle it has a known methodological flaw, described below. Read "Don't repeat these" before
forming a theory.

## The question

Container starts intermittently failed with `container port N was not published`. The port map
showed the port key present with **nothing bound** (`9000/tcp=[]`) on a container the daemon
reported as *running*. It happened roughly once in 750 starts, across four different proofs, and
looked exactly like a Docker Desktop defect.

The question is now narrower and sharper:

> Is there any real container-runtime port-binding defect at all, or is every instance so far
> something prova misread?

As of this writing, **no confirmed runtime defect has been observed on either runtime.** One
instance was proven to be ours. The other is unattributed.

## Settled: a container that exited is not a runtime that failed to bind

The daemon **clears a container's port bindings when it stops**. A short-lived container therefore
presents identically to the defect being hunted: port requested, nothing bound. prova believed it,
recreated a container that had simply finished its job, and recorded the waste as evidence against
the runtime.

Controlled, one variable at a time — same runtime, same concurrency, 800 starts:

| container lifetime | defects |
|---|---|
| `sleep 2` (≈ scan budget) | 7 |
| `sleep 30` | 0 |
| `sleep 2`, after fix | 0 (and ~20s faster, for want of pointless recreates) |

Fixed in `modules.rs` (`start()`): if the port map is empty *and* the container has exited, that is
a finished container, not a defect. Locked in by
`modules::docker::tests::a_container_that_exited_is_not_counted_as_a_runtime_defect`.

**This one is solid.** Mechanism identified, one-variable confirmation, regression test.

## Open: the grpcbin case

The original production failures were `moul/grpcbin` on port 9000 — an **amd64-only image running
emulated on arm64**, a **long-lived server**, reached **after readiness had already passed** (the
server was listening *inside* the container while no host mapping existed). None of that is
explained by the exited-container bug.

Aimed at that exact workload on Docker Desktop, 8 workers × 25:

| arm | defects |
|---|---|
| prova / bollard | 8 / 200 (4%) |
| `docker` CLI, same protocol | 0 / 200 |

Then, with no relevant change, the rate collapsed to ~0/200 and would not reproduce. See the flaw
below before trusting either number.

## The methodological flaw — fix this first

**The arms ran sequentially, minutes apart, against a daemon whose behaviour drifts.** The measured
defect rate moved from ~8/200 to ~0/200 across a few minutes with no code change that mattered. A
drifting baseline can manufacture exactly the prova-vs-CLI asymmetry that was reported, so the
grpcbin comparison does not currently support a conclusion.

**The fix: interleave the arms inside a single run** so both clients meet an identical daemon
condition — alternate prova and CLI starts within each worker, or pair them one-for-one. Then a
difference cannot be drift. This is the prerequisite for any grpcbin conclusion, and is the first
task.

Second, add a **native-arm64 gRPC image** as a third workload. If the defect follows *emulation*
rather than the client, the framing changes again.

## Don't repeat these — already refuted

| Hypothesis | Test | Result |
|---|---|---|
| Our polling starves the daemon (2 inspects per 50ms per container) | halved to one | **still 8 defects** — refuted; the fix was kept anyway, it is free |
| Empty binding is transient; 2s budget too short | widened to 10s | **still 7 defects** — the map stayed empty 10s on a *running* container |
| Bollard's pinned API version (1.47 vs daemon 1.51) | queried both versions for the same container | **identical port maps** — not a cause, though negotiation was added and is correct |
| Sequential volume reproduces it | 1200 sequential starts | **0 defects** — concurrency is required |
| Docker Desktop is at fault | CLI arm, same protocol, same runtime | **0 defects via CLI** — but see the flaw above |

## Facts you need, that are easy to get wrong

- **`DOCKER_HOST`, never `docker context use`.** Bollard reads `DOCKER_HOST` or the default socket
  and knows nothing about Docker contexts. Switching context moves the CLI *only* — which would
  point the two arms at different daemons and silently compare a runtime against itself. Verified:
  with `DOCKER_HOST` set per socket, a container appears on that daemon and only that one, and
  `shell.run` inherits the variable so the CLI arm follows it too.
  - OrbStack: `unix://$HOME/.orbstack/run/docker.sock` (it owns `/var/run/docker.sock` on this box)
  - Docker Desktop: `unix://$HOME/.docker/run/docker.sock`
  - **OrbStack is the default**, so a soak that forgets `DOCKER_HOST` measures the healthy runtime
    and reports a clean bill of health — including when checking for a regression.
- **Select with `-k`, not by path.** An explicit path bypasses the manifest, and the companion that
  registers the `soak` capability goes with it, so `prova proofs/soak` always skips.
- **`docker.diagnostics()` counters are process-wide**, so per-worker deltas overlap under
  concurrency. The max across workers approximates the run total; per-worker `usable` counts are
  exact.
- **A recovery is silent by design.** Without the counters, "N starts, all fine" and "N starts, k of
  which the runtime botched and we healed" are the same observation.

## How to run it

```sh
# Both arms, current runtime
PROVA_SOAK=1 PROVA_SOAK_WORKERS=8 PROVA_SOAK_STARTS=800 prova -j 8 -k "soak "

# One arm, aimed at a specific runtime
PROVA_SOAK=1 PROVA_SOAK_CLIENT=cli PROVA_SOAK_WORKERS=8 PROVA_SOAK_STARTS=800 \
  DOCKER_HOST="unix://$HOME/.docker/run/docker.sock" prova -j 8 -k "soak "

# The grpcbin workload (emulated, long-lived, with readiness)
PROVA_SOAK=1 PROVA_SOAK_IMAGE=moul/grpcbin PROVA_SOAK_PORT=9000 \
  PROVA_SOAK_LIFETIME=none PROVA_SOAK_WAIT=1 \
  PROVA_SOAK_WORKERS=8 PROVA_SOAK_STARTS=200 prova -j 8 -k "soak "
```

Knobs: `PROVA_SOAK` (opt-in gate), `PROVA_SOAK_CLIENT` (`prova`|`cli`|`both`), `PROVA_SOAK_WORKERS`,
`PROVA_SOAK_STARTS`, `PROVA_SOAK_IMAGE`, `PROVA_SOAK_PORT`, `PROVA_SOAK_LIFETIME` (`none` = run the
image's own entrypoint), `PROVA_SOAK_WAIT`.

## Where the code is

| What | Where |
|---|---|
| Port classification (pure, unit-tested) | `crates/prova-core/src/modules.rs` — `classify_port`, `PortState` |
| Scan + retry + counters | same file — `published_ports`, `start()`, `PORT_BIND_RECOVERIES/FAILURES` |
| Fault injection (crate-internal, never parsed from Lua) | `Spec::fault_empty_binding` |
| Rust tests incl. regression | `modules::docker::tests` |
| Diagnostics contract | `proofs/docker/diagnostics_test.lua` |
| The soak instrument | `proofs/soak/port_binding_soak_test.lua` (its header carries the findings) |
| Soak gate capability | `.prova/config.lua` |

## Standing decision to revisit

The recovery machinery (spaced recreate on an empty binding) is correct, tested, and cheap — but
**no confirmed instance of the defect it was built for has been observed.** Treat it as an unproven
contingency, not a known-necessary workaround. If the interleaved experiment shows the grpcbin case
is also ours, the honest move is to delete the recovery rather than keep a workaround for a defect
that never existed. Keep `docker.diagnostics()` either way; it is what makes any of this measurable.

## Repo state

- The docker/annotations/soak work through `style: cargo fmt crates/prova-core/src/plugins.rs` has
  been **pushed** and is immutable. The three commits below it (exited-container fix, poll-load fix,
  this document) are local and sit on top of `feat(plugins): private plugin dependencies`.
- `proofs/plugins/isolation_test.lua` **now passes.** It was a deliberately-red spec from
  `prova-agents@`; the feature it specified has since landed, so the whole suite is green. If you
  read an older note calling it red, that note is stale.
- Rust: 164 passing. Proof suite: 17 passing, 0 failing, 2 skipped (the soak arms, correctly gated
  off without `PROVA_SOAK`).
- `cargo fmt --check` clean workspace-wide.
- Other workspaces (`prova-agents@`, `prova-mocks@`) are active on this line — check
  `jj workspace list` before restructuring anything, and prefer rebasing your own commits over
  moving theirs.
