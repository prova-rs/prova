# Agent Ergonomics — frictions from the first external dogfood

Drafted 2026-07-16. Records what an **agent** actually hit driving Prova against a real target
(Minion — a local macOS daemon, deliberately *not* a container), and what to fix. This is the
"agentic PDD" requirement stated by the principal: *Prova must fit naturally into an agent's
toolkit, and must be learnable without reading its source.*

Ordered by cost. Every claim below is an observation from the session, not a hypothetical.

---

## 0. The meta-friction: Prova is not self-discoverable

**An agent learning Prova today must read Prova's source.** In this session, learning enough to
write one package required: `crates/prova-core/src/modules.rs` (to learn the `shell`/`fs`/`docker`
shapes), `packages.rs` (resolution order), four `docs/design/*.md` (doctrine), and `library/*.lua`
(the LuaCATS stubs). **None of that is reachable from inside the environment being driven.**

Evidence, in the order it cost round-trips:

| What happened | What it cost |
|---|---|
| `shell.run({ "bin", "--help" })` → *"error converting Lua table to String"* | A failed call. The argv form is `container:run`'s, not `shell.run`'s — discoverable only by reading source. |
| `shell.run(...)` → `ShellResult: 0xaa780dcf8` | Field names (`stdout`/`stderr`/`code`) **guessed**, then probed with a `for k in pairs()` loop. |
| `ctx:tempdir()` — path string or handle? | A probe. |
| `prova.workspace` | A probe, to learn it has only `create` (not the project-root resolver the name suggests). |

Each was a round-trip that one `help()` call answers. **The LuaCATS stubs are for the IDE (a human,
in an editor). They are not available to an agent driving `prova eval`.** Both audiences are real;
today only one is served.

**The portfolio is inconsistent, and Prova is the outlier:**

| | In-environment introspection | MCP tool |
|---|---|---|
| Substrate | `cos.help.*`, `cos.list()`, `cos.namespaces()` | ✅ `introspect` |
| Minion | `minion.help()` — filterable (`minion.help("layers")`) | ✅ `introspect` |
| **Prova** | **none** | **none** — `eval` · `run` · `list` · `up` · `down` · `status` |

Both siblings already solved this, and their MCP instructions *lead* with it ("DISCOVER:
`minion.help()` lists every function"). Prova's MCP instructions instead lead with a hand-written
one-screen summary — which is excellent, but it is a *snapshot* that drifts, not the surface itself.

### Fix 0.1 — `prova.help([filter])`, the in-environment surface

Mirror Minion's shape (it is the closest sibling and it works):

```lua
prova.help()            --> every entry: { name, signature, summary }
prova.help("shell")     --> filtered by substring across name/summary
```

Returned as data (a table), not printed prose, so an agent can filter it and a test can assert on
it. Cover **every global an author can touch**: `prova.*`, `shell`, `fs`, `net`, `docker`, `http`,
plus the `Context` methods (`use`/`manage`/`defer`/`tempdir`) and the matcher vocabulary — the four
things above that cost probes are all in that set.

**Generate it from the same IR that emits LuaCATS**, or the two drift by construction and we have
shipped the bug twice. LuaCATS (`library/*.lua`) and `help()` are two renderings of one truth:
the stub serves the editor, `help()` serves the runtime. One source, two sinks.

### Fix 0.2 — an `introspect` MCP tool

Prova's MCP has no way to ask what exists; `eval` is the only door, so discovery is
`eval("for k in pairs(fs) do ... end")` — spelunking through a tool meant for probing *behaviour*.
Add `introspect` (filter optional), returning `help()`'s data. This is the one tool both siblings
have and Prova lacks.

### Fix 0.3 — return values should be self-describing

`ShellResult: 0xaa780dcf8` tells an agent nothing. Userdata that crosses the Lua boundary should
answer `tostring()` with its shape (`ShellResult{ code=0, stdout=42B, stderr=0B }`) and, ideally,
expose `__pairs` so `for k,v in pairs(r)` enumerates fields. Cheap; removes a whole probe class.

---

## 1. `shell.run` / `shell.spawn` have no argv form

`container:run{ "env", "PGPASSWORD=…", "psql", "-tAc", sql }` takes **argv** — the ecosystem doc
sells this explicitly: *"no shell, no quoting."* The **local** `shell.run(command, opts)` takes only
a command **string**, so the same package, doing the same job against a local binary instead of a
containerized one, must hand-quote.

This bit immediately: passing arbitrary Lua source to `minion eval "<lua>"` is unquotable in
general (quotes, newlines, `$`). The workaround was to write the payload to a temp file and pass a
path — i.e. **route around the API**. Any package driving a local CLI with user content (SQL, JSON,
scripts) hits this.

**Fix:** accept an argv table in `shell.run`/`shell.spawn`, exactly as `container:run` does — same
rationale, same shape. Keep the string form (it is ergonomic for fixed commands).
`shell.run({ "minion", "eval", src }, { env = … })`. *This is an asymmetry between the local and
containerized halves of one SDK, and the containerized half is right.*

---

## 2. No project/manifest root primitive

A repo-local package must locate repo artifacts — `target/debug/miniond`, fixtures, testdata. There
is no primitive for "where is the manifest / project root". The options were: hardcode an absolute
path (unshippable), or depend on the process cwd (worked — the MCP runs at the repo root — but it
is an undocumented coincidence, and CI or a nested run breaks it).

`prova.workspace` exists and, by name, looked like the answer; it exposes only `create`.

**Fix:** expose the resolved package root (home) (e.g. `prova.root`, or `ctx.root`) — the directory
everything resolves against. One field. It is the anchor every path in a repo package needs, and the
runtime already knows it (it resolved the manifest to get here).

---

## 3. The Resource shape assumes Docker; a local daemon is a real shape

`prova.containerized` is the only constructor, and `ecosystem.md` is careful that the Resource shape
is "one shape, not the definition of a package". But **Minion is a Resource in every way that
matters** — provision → wait for readiness → manage teardown → return a client — and it **cannot be
containerized** (it needs macOS, TCC grants, and real HID devices). So `containerized` cannot help,
and the boilerplate comes back by hand:

```lua
local dir  = ctx:tempdir()
local proc = ctx:manage(shell.spawn(bin, { env = hermetic_env(dir) }))
local client = prova.retry(function() return connect(dir) end, { timeout = "20s" })
return { client = client, sock = …, proc = proc }
```

That is `containerized`'s body with `shell.spawn` where `docker.run` was. The doc's own test for a
new constructor — *"a shape proves to carry recurring boilerplate"* — is met the moment a second
local-daemon package exists.

**Fix (proposed, smallest first):**
- **(a)** Generalise the trio's third slot: `{ client, url, handle }` where `handle` is a container
  **or** a process — both already answer `stop()`. `container` stays as an alias for the Docker case.
- **(b)** Add a `prova.local_service{ … }` constructor (spawn + wait + manage + trio) mirroring
  `containerized`'s spec table (`bin`, `env`, `url`, `client`, `wait`, `timeout`).

`net.free_port`'s doc comment ("for a locally `shell.spawn`ed app") shows the local-service case was
already anticipated — it just has no constructor. *Note this shape is also un-Dockerable in
principle, not just in practice: it is the case Prova's black-box doctrine cannot route around,
because the thing under test **is** the local machine's integration.*

---

## 4. ~~Manifest discovery walks up only~~ — **WRONG. Already implemented.**

> **Retracted 2026-07-16, the same day it was filed.** `home::find` already checks, at each ancestor,
> the directory itself **and** its `prova/` / `.prova/` child — `Home` documents that `home.dir` is
> the root: where everything (including `.luarc.json`) resolves and the editor attaches, whether
> `prova.toml` sits in the root itself or tucked in its `prova/` / `.prova/` child. Verified empirically: a tree containing only
> `prova/prova.toml` + `prova/tests/` is discovered and run from the repo root.
>
> **How the error happened, because it is the thesis in miniature.** I read `package-system.md`
> ("found by walking up"), inferred a limitation, and filed it — without testing. I could not *ask*
> the system what it did, so I guessed from prose, and guessed wrong. **A false bug report is the
> same failure mode as a wasted probe**: both are what an agent does when the surface cannot answer
> for itself. It is fitting that the one friction I invented is the one §0 predicts.
>
> Kept, not deleted: a retraction is a load-bearing part of a friction log. The original claim
> follows, struck.

~~The emerging convention is a `prova/` directory as a project standard; discovery walks **up** from
cwd looking for `prova.toml`, so `<repo>/prova/prova.toml` is invisible.~~ **False — it is found.**



The emerging convention (principal, 2026-07-16) is a **`prova/` directory as a project standard**:
local packages, the manifest, suites, and topologies in one place —

```
<repo>/prova/
  prova.toml
  packages/<name>.lua
  tests/*_test.lua
  topologies/
```

Discovery today walks **up** from cwd looking for `prova.toml`, so `<repo>/prova/prova.toml` is
invisible from `<repo>` — the exact layout the convention wants. (`.cargo/`, `.github/`, `.claude/`
all solved this the same way.)

**Fix:** during the walk, at each level check `./prova.toml` **then `./prova/prova.toml`**. Keeps
the root-manifest layout working, makes the directory standard discoverable, and lets `[run] proofs`
and `[dependencies]` resolve from the package root (home) (which they already do).

---

## Status

- **None of these are blockers** — every one had a workaround, and the hermetic Minion daemon *was*
  provisioned and torn down correctly through the existing API (`ctx:tempdir` + `shell.spawn{env}` +
  `prova.retry` + `ctx:manage` all behaved exactly as documented). The frictions are about **cost to
  learn** and **cost to route around**, not capability.
- **Fix 0 (discoverability) is the one that matters.** It is why the others were found slowly, and
  it is the difference between an agent using Prova and an agent reverse-engineering it.
- **Shipped 2026-07-16:** **0.1** `prova.help([filter])` · **0.2** the `introspect` MCP tool ·
  **1** argv for `shell.run`/`shell.spawn` · **2** `prova.root` / `prova.home`. **4** was retracted
  (already implemented). One correction to 0.1 as specced: there is **no IR** — the LuaCATS stubs are
  hand-written and `annotations.rs` embeds+syncs them, so the *stub* became the single source and
  `help()` a second sink off it (`CORE_STUBS` moved to `prova_core::help`, embedded once, consumed by
  both). That is better than the proposed registry: a registry would have been a second place to
  write every summary.
- **Remaining: 3** (`local_service`) — deferred until a second local-daemon package proves the
  boilerplate recurs, which is the doc's own bar for a new constructor.

---

# Round two — 2026-07-24 (the same target, a later session)

Same dogfood, ~40 proofs in. Round one was about *learning* Prova; these are about **being misled by
it** — three of the five cost debugging time on a system that was already correct, which is the most
expensive kind of friction there is.

## 5. `prova.retry` reported a stale error, and never named the real cause — **FIXED**

**Cost: ~20 minutes and two unnecessary "fixes" to the target system.** A proof waited for an
orphaned child process to exit:

```lua
prova.retry(function()
  local r = shell.run({ "kill", "-0", tostring(pid) })
  assert(r.code ~= 0, "process outlived its parent")   -- no return!
end, { timeout = "15s" })
```

The closure asserts and returns nothing, so `retry` never saw a truthy value and spun to the
deadline — on a condition that was *already met within 3 seconds*. Two things then compounded:

1. **`last_err` was sticky.** It was set when the assert failed early and never cleared, so the
   timeout reported `(last error: process outlived its parent)` — an error from twelve seconds
   earlier, presented as the current state. That is worse than no detail: it is confidently wrong
   detail, and it is what sent me to "fix" the system twice more. (Both fixes turned out to be
   independently necessary, which is luck, not vindication.)
2. **"condition not met" does not distinguish** "your system never got there" from "your closure
   never returned anything". The second is the commonest authoring mistake in this API — LuaLS even
   flags it (`missing-return`) — and the runtime said nothing about it.

**Fixed here.** A falsy return now clears `last_err` (an error that stopped happening is not the
current state), and a timeout with nothing raised says: *"the closure never returned a truthy value —
`retry` waits for a TRUTHY RETURN, so a closure that only asserts must end with `return true`"*.
Proofs: `proofs/spec/utilities/retry_test.lua`.

**The general lesson, worth applying elsewhere:** when a primitive can fail for two structurally
different reasons, saying only "it failed" makes the caller debug the wrong one. Prova is already
excellent at this in its assertion messages; its *polling* primitives were not.

## 6. `learn` told a package with three packages that it had none — **FIXED**

`prova learn project` and `learn doubles` both rendered:

> **Declared packages**: none — add them under `[dependencies]` in the manifest.

while `require("minion")`, `require("policy")` and `require("lib")` all worked — three local packages
under the declared `[run] packages`. The line reads `[dependencies]` (external sources) only.

It is a true statement about one manifest key and a **false answer to the question being asked**.
`learn` exists so an agent need not read the source; for a package whose entire vocabulary is local
packages, it actively denied that vocabulary existed. Worse in `doubles`, where the sentence lands
directly under "Packages declared in this package add their facets to the vocabulary" — the local
`minion.daemon(ctx)` *is* such a facet.

**Fixed here.** Both kinds are listed, because `require("<name>")` does not distinguish them:

```
**Packages** (`require("<name>")` in any proof):
  lib     local (.prova/packages/lib)
  minion  local (.prova/packages/minion)
  policy  local (.prova/packages/policy)
```

## 7. The MCP surface cannot select by path, and swallows `t:log`

Two parity gaps, both hit while driving Prova **only** through MCP — which is the intended agent mode.

**(a) No path selection.** The CLI takes `prova <file-or-dir>...`; the MCP `run` tool takes
`keywords` / `nodes` / `tags` / `specs` / `profile` / `jobs` but no `paths`. So "run this one proof
file" — the most natural unit an agent works in — has no MCP expression. `-k <topic>` is not a
substitute: keywords match the node PATH (test names), so `keywords: ["appscript"]` selected 1 of the
4 tests in `appscript_test.lua`, which reads as a broken filter until you know why.
**Fix:** add `paths: string[]` to the MCP `run` schema, forwarding to the same argument the CLI takes.

**(b) `t:log` output is invisible.** A proof logged a computed coverage number
(`t:log("489 Commands, 213 drivable")`) — deliberate, load-bearing diagnostic output. The MCP result
is `{passed, failed, skipped, duration_ms, failures[]}`, so it was simply gone; I had to shell out to
the CLI to read my own proof's output, which defeats the point of the MCP.
**Fix:** carry per-node `logs` in the MCP result (at minimum for failures; ideally always — an agent
asked for them by writing `t:log`).

## 8. No WebSocket (or raw TCP) client — a whole class of SUT is undrivable

Prova can stand up `http.mock` and `grpc.mock`, and `http.client` drives a real service. There is no
equivalent for **WebSocket**, and localhost-WS is how a growing class of desktop integrations talk:
this target has two of them (a browser extension bridge and an in-Photoshop UXP panel), both
daemon-as-server / package-as-client.

The concrete loss: a proof cannot stand in as the panel, so the full chain — Lua package chooses the
bridge → daemon → extension → WS → panel → reply — is provable only in Rust unit tests, one process
at a time. The black-box proof stops at the process boundary, which is exactly the boundary Prova
exists to cross.

**Fix (in rough order of value):** `ws.client(url)` with `:send`/`:recv`/`:close` — enough to *be* the
peer, which is the common case for testing a bridge. `ws.mock(ctx)` (serve, and assert on a journal
like `http.mock`) is the natural sibling but strictly less urgent: the SUT is usually the server.

---

## Round-two status

- **5 and 6 are fixed in this workspace** (with proofs / unit tests). Both were *misleading output*
  rather than missing capability — cheap to fix, disproportionately expensive to hit.
- **7 is small and mechanical**, and it is the difference between the MCP being a first-class surface
  and being a lossy subset of the CLI. An agent that has to shell out to `prova` to read its own
  proof's log is not really driving the MCP.
- **8 is a genuine capability gap** and the only one that needs design. It is also the one that would
  have let this session prove its most interesting claim end-to-end instead of at a seam.
- Round one's remaining item (**3**, `local_service`) is *still* unresolved and now has its second
  data point: this session provisioned the same hermetic daemon the same way. That is two local-daemon
  packages' worth of identical boilerplate — the doc's own bar for a constructor is met, if the second
  witness counts.

---

# Round three — 2026-07-24 (cross-repo integration: Minion consuming Aegis)

Driving a genuine two-repo integration: Minion's proofs reuse the sibling **Aegis** repo's own
`aegis` prova package (a hermetic Gate Authority + its CLI), declared cross-repo via
`[dependencies] aegis = { path = "../aegis/.prova/packages/aegis" }`. This is exactly the "packages compose
across projects" story, exercised for real for the first time.

**The cross-repo MCP flow itself was frictionless** — worth stating, since the concern was that it
might not be. The Prova MCP was started in the Minion repo; `run` / `learn` / `introspect` all drove
the *Aegis* package cleanly via the `package` parameter (resolved fresh, ran the other repo's suite).
Nothing about targeting a second package by path got in the way.

## 9. A package could not locate ITS OWN repo — **FIXED (`plugin.dir`)**

The one real friction, and a sharp one. The `aegis` package needs to spawn `<aegis>/target/debug/aegis`.
It resolved that as `prova.root .. "/target/debug/aegis"` — correct when Aegis runs its own suite, but
**`prova.root` is the *consuming* package's root**, so the moment Minion consumed the package it
resolved `<minion>/target/debug/aegis`, which does not exist. A package reused cross-repo had *no
anchor on its own location* — only on the consumer's.

The workaround was ugly (pass `bin_dir` explicitly, computed from a `prova.root .. "/../aegis"`
sibling-layout guess — unshippable and repo-arrangement-dependent). The right fix is a primitive: a
package must be able to find itself.

**Fixed here.** Every package chunk now runs in a per-package environment carrying **`plugin.dir`** —
the directory its own file lives in (`packages.rs`, `plugin_env`). So the `aegis` package resolves its
binary as `plugin.dir .. "/../../../target/debug/aegis"` and works **wherever it is consumed**, its
own suite or another repo's. The Minion integration proof then needs *zero* configuration:
`aegis.daemon(t)` just works. Proof: `proofs/packages/plugin_dir_test.lua` (own dir is the package's
home, distinct from `prova.root`); verified end-to-end by Minion's `gate_attach_test` running with no
`bin_dir`.

Design note: it's a *per-package* binding, not a global, set the same way the private-dependency
`require` is (raw-set into the chunk env whose metatable falls through to the real globals) — so it
cannot leak to consumers, and a package without private deps now still gets its own env (previously
only packages *with* private deps did). `plugin.dir` is the minimal primitive: the package's repo root,
fixtures, or binaries are all `plugin.dir .. "/…"` from there.

---

# Round four — 2026-08-12 (Substrate kernel-extraction dogfood: prova as the orchestration gate)

Substrate now runs prova as the sole quality authority inside a multi-model orchestration loop:
an orchestrator authors promises, spawns a Coder agent per slice, and gates every result
mechanically. Full `run all` (ut + quality + session lanes) is ~434s on that workspace; the
slice-scoped acceptance (the two presence proof files + their crate-scoped deputy conducts) is
seconds. That gap is the friction: the loop wants per-slice gates, and today there is no
first-class way to name one.

## 10. No claim-scoped selection — "run the acceptance for THIS slice" needs a selector

<!-- claim: claim-scoped-selection recorded=2026-08-12 -->
**A definition of done is a selection string: `--covering <claim>` selects exactly the proofs
whose `covers` discharge it.** The axis speaks three grains — a full address
(`docs/x.md#id`), a bare id, and a whole doc path (every claim in that spec) — repeatable,
composing with every other axis and with `--list`, spoken identically on the CLI and the MCP
tools (the selection-parity gates hold the surfaces equal), and visible to deputies as
`prova.selection.covering` so a slice-aware conduct can narrow further. A spawn brief
therefore names its gate mechanically — `prova --covering docs/specs/PRESENCE_KERNEL.md` —
the orchestrator executes it after every Coder pass whether or not the model remembered a
verify step, dependencies of the selected proofs are pulled in as with any selection, and
the run records a NARROWED deputed account so partial evidence never wears full's face. The
expensive full sweep retreats to the sprint boundary where it belongs. (Field evidence, the
presence slice: inner gate ≈ 5s vs `run all` 434s — 10-100× too heavy per Coder iteration,
with the workspace-wide conduct surfacing unrelated flakes as false reds against the slice.)

## 11. Count-threshold structural assertions rot within hours (observation, not an ask)

A structural proof asserted `grep -c "resolve(&PresenceInput"` ≥ 8; a same-day de-clone
(three inline gatherings → one helper) legitimately dropped the count to 7 and turned the
proof red against a change that *improved* the property it guards. Authoring practice, not a
prova feature: bind structural proofs to presence/absence facts (the dependency edge, the
deleted symbol, ≥1 kernel call) and leave slack on any count. Recorded here so the next
package author inherits the scar without the burn.

<!-- claim: checklist-archetype-stale-claims-table recorded=2026-08-12 -->
A manifest carrying the pre-rename `[claims]` table is **honored with a warning** naming `[specs]`,
like every other retired spelling (docs/design/manifest.md#deprecated-spellings-teach) — so its docs
are scanned and its anchors resolve, instead of every `covers` reporting DANGLING ("no anchor
exists") while the prose sits right there in the file. Silently ignoring a whole section is the
manifest layer's version of a dropped option: the author declared the spec source, prova agreed, and
nothing was read. Found via the checklist init archetype, which still scaffolds `[claims]`; the
bridge fixes every generated copy at once, and the archetype's own regeneration is cleanup, not the
fix. Retires at 1.0 with the other pre-1.0 spellings.

<!-- claim: buildkit-wedge-hangs-suites-silently recorded=2026-08-13 -->
**A tool that never answers at all is a third failure mode, and `first_byte` is its bound.** The
existing clocks cannot express it: `idle_timeout` asks "is it still alive?" and a build that has
gone quiet mid-step is legitimately alive, while a wall `timeout` must be sized for the slowest
honest build and so answers minutes or hours late. Time-to-FIRST-byte is the one interval a caller
can bound tightly without knowing the work: a healthy builder prints `load build definition` in
about a second, and a wedged buildkitd prints nothing, ever. So `shell.run { first_byte = "…" }`
kills a conduct that produced no output on either stream within the window, `docker.build` carries
a default one, and the error says the tool never answered and names the fix (restart the builder)
rather than reading as a slow build. Composes with conduct-heartbeat-not-deadline: three clocks,
three different questions — has it started? / is it alive? / may it keep going? — and the first byte
disarms this one for good. (Field evidence: a wedged Docker Desktop buildkitd, with `docker pull`
and every other daemon op healthy, hung each image-building suite until its outer bound — 2h per
suite, serially, in the workspace sweep.)

---

# Round five — 2026-08-13 (concurrency stress: eight run-alls, parallel mid-edit conducts, aborted holders)

A full day of multi-agent orchestration put the flock discipline under real load for the first
time: eight `run all` sweeps, hand-run `-p` conducts interleaved from a second invocation,
deputy conducts inside delegated verifications, two agent processes killed mid-work. The core
held: **zero** cargo artifact-dir backstop hits ("Blocking waiting for file lock") across every
log — conducts never overlapped; killed holders released instantly (no leaked processes, no
stuck locks); deputy-owned junit copies stayed distinct under the shared-profile-path pattern
(each conduct+copy runs inside its lock window). Two frictions:

## 12. Lock waits are invisible — a queued conduct reads as a slow one

<!-- claim: narrate-lock-waits recorded=2026-08-13 -->
**A queued conduct says it queued, and the run banks what contention cost it.** Waiting is
narrated at every seam that can wait — with its duration, in one vocabulary — and the run records
`run.lock_wait_ms` in the `timings` set, so contention is a metric a reminder can watch drift on
and a baseline can hold, not a number an operator reconstructs by cross-referencing sibling logs.

There are **four** seams, and they are not equivalent (2026-08-13, from the code rather than the
log — the field report's 848.8s-for-190s-of-work was never attributed to one of them):

- the scheduler's pre-start queue (`run_plan`'s non-blocking `try_acquire`), which falls OUTSIDE
  the waiting unit's duration — a unit's timer starts only once the scheduler admits it;
- the `Scope.Run` single-flight wait inside `t:use`, which falls INSIDE the reader's duration and
  alone explains a unit reading 848.8s for 190s of work, with no flock involved;
- the blocking `locks::hold` behind a `[runner] locks` provision;
- the `prova lock <token> -- cmd` wrapper, which said it was waiting but never for how long.

Two rules keep the two channels honest. **Durations keep meaning wall time** — annotate the wait
beside a duration, never redefine the duration, because baselines and tolerance bands already
compare against it. And **`run.lock_wait_ms` counts only wall time the process was STALLED** on a
cross-instance lock: the time that would be given back if the contention vanished. Summing
per-leaf waits would double-count two leaves blocked on one token and could exceed the run's own
wall clock, which makes a metric useless exactly when contention is worst; a wait overlapped with
other work costs nothing and is narrated (so it is diagnosable) without being banked (so the
number stays comparable). The `Scope.Run` wait is narrated for the same reason and likewise not
banked: it is intra-run coordination, not contention with another instance — the conducting worker
is working.

## 13. Two deputies with identical conduct scope run the same cargo twice

<!-- claim: dedupe-identical-deputy-conducts recorded=2026-08-13 -->
**Two conducts with the same identity are one execution.** A `Scope.Run` fixture may declare what
its conduct depends on — `identity = { inputs = { … } }`, package-relative paths or globs — and the
run-wide store keys on the resulting digest as well as on the fixture's name: whichever consumer
asks first conducts, and a second fixture with a DIFFERENT name and the SAME identity adopts that
value instead of re-running the tool. `fs.digest(paths)` is the primitive behind it, in the belt
rather than shelled out (docs/plans/incremental-prova.md), because `git hash-object` and
`sha256sum` are absent on a bare Windows runner and a package that computes identities by shelling
out is a package that works on its author's box. The digest is over file CONTENTS and
package-relative paths, sorted, `/`-separated — the same tree answers identically on every OS, and
a missing path is part of the answer rather than an error, because absence changes the build.

Names stay the isolation boundary (each deputy still owns its artifact copy); identity is only
about execution. A fixture that declares nothing behaves exactly as before — conducted once per
run, keyed by name alone. (Field evidence: two proof files each conducting
`cargo nextest run -p cos-systems-lua -p cos-daemon`, same packages and profile, paying ~100-140s
twice per sweep for two 906-case junit copies differing only in filename.)

## 16. One invocation, one package set — reconciliation included

<!-- claim: reminder-reconcile-ignores-adhoc-packages recorded=2026-08-13 -->
The post-run reconciliation pass — which re-executes proof files to collect their `covers` for the
attention account's `owed` — resolves the **same** package set the run resolved, `-P name=source`
layering included. Two resolution answers inside one invocation is the defect: a file that collected
and passed against the ad-hoc package died in the reconcile pass on a function only that package has
("reminders not evaluated — could not reconcile the ledger: attempt to call a nil value"), so the
run was green and its attention account silently stale. The failure mode is inherent to a pass that
re-executes rather than reuses, and the rule that contains it is that resolution is a property of the
invocation, not of the phase. (Field evidence: the archetype-fleet dev-pin work, where `-P` pointing
at a working copy is the normal way to drive a package under edit.)

## 14. An option prova cannot honor is refused, never dropped

<!-- claim: unknown-test-opts-silently-ignored recorded=2026-08-13 -->
An unknown key in a unit's `opts` table (`prova.test`/`group`/`flow`/`step`, and `suite.config`)
is **refused at collect time**, naming the key, the nearest accepted spelling, and the accepted
set — never silently dropped. A **removed** spelling names its successor instead: `spec = { … }`
(deleted in v0.18.0, gone-not-bridged) says an open proof is flagged `promises` and its obligation
is addressed by `covers`. The manifest layer already holds this line (`deny_unknown_fields` plus a
did-you-mean); the DSL holds the same one, because a dropped option is worse than a rejected one —
it reads as configured.

The field evidence: every suite still carrying `spec = { … }` had its *tolerated* open specs
degrade into hard failures the moment the key stopped being read (all 8 p6m-run operators, found
2026-08-12 by the workspace sweep). A typo has the same shape — `tiemout = "10m"` silently means
"no timeout", and the suite that thought it was bounded is not.

## 15. The collect/runtime boundary teaches instead of panicking

<!-- claim: collect-time-shell-panics-raw recorded=2026-08-12 -->
A runtime-only surface reached from **collect-time** code — a proof file's top level, where the plan
is built — raises a teaching error naming the boundary and what collect time *does* have (`fs`,
`toml`/`json`/`yaml`, `env`: pure reads), never a raw tokio panic. The panic was worse than ugly:
unwrapped it aborted the run mid-collect, and inside `pcall` it was *caught*, so a file could print
a reactor panic and still report green.

The boundary is deliberate, not incidental. Collect answers "what units exist?" for `--list`,
`tests`, `--covering`, and every MCP query; if a plan could shell out, then listing a suite would
execute arbitrary commands, and selection would stop being cheap and safe. So process, network, and
container work belongs in a fixture or a test body — where the runtime, its bounds, and its lease
all exist.

<!-- claim: module-opts-silently-ignored recorded=2026-08-13 -->
**The same rule holds one layer over: a MODULE option prova cannot honor is refused, never
dropped.** `shell.run`/`shell.spawn`, `docker.build`, `docker.run` and its nested `wait`, the
`http` request/client/`wait_for` options and `graphql.client` all parse by key lookup, so every one
of them used to read a typo as *configured*. They now share the unit surface's gate
(`crate::opts::Closed`, one implementation for both layers), naming the key, the nearest accepted
spelling where one is close enough to name, and the accepted set.

**Version skew is why this matters more here than at the unit layer.** A proof written against a
newer prova — `docker.build{ first_byte = "90s" }` — ran on an older binary that had never heard of
the option, dropped it, and passed while proving nothing about the bound it named (found 2026-08-13
writing the first-byte proof; only the conductor-vs-subject discipline caught it). Refusing turns
that into the loud, accurate "this proof needs a newer prova" it always was.

**Some wrong calls deserve more than a denial.** `shell.spawn("kubectl", { args = {…} })` ran
kubectl with NO arguments (field evidence 2026-08-14, prova 0.22.0, ybor-studio topology): `spawn`
reads `cwd` and `env` only, so `args` was dropped, and `spawn` is the worst possible host because
the process still STARTS — a bare `kubectl` printed usage into a discarded stdout and the run
failed minutes later waiting for an effect nobody had requested. `args` is what every other process
API in the world takes and nothing in `{cwd, env}` is near it, so nearest-spelling has nothing to
offer; the refusal therefore teaches the argv form (§1) outright. The same mechanism carries
removed spellings, which is the identical failure from the other end.

<!-- claim: http-wait-for-cannot-authenticate recorded=2026-08-14 -->
**A readiness poll can authenticate.** `headers` is a `WaitOpts` key, so `http.wait_for(url, {
status = 200, headers = { Authorization = … } })` waits on a health endpoint behind auth, and on
`client:wait_for` a per-call header layers OVER the client's defaults by name — the same precedence
an ordinary request gets, so the two verbs cannot disagree about whose `Authorization` wins.

Without it the free verb sent no headers at all, so a guarded endpoint could only ever answer 401
and the wait died on its deadline. That is the expensive part: the failure arrives as "did not come
up in 30s", a diagnosis pointing at the service rather than at the request, and the way out was a
hand-rolled `http.get` retry loop — the reach-for-another-tool pressure this module exists to
remove. `client:wait_for` carried the client's defaults all along, so the two verbs disagreed about
whether polling could authenticate at all.

Found by the closed-set audit rather than in the field, which is worth recording: the LuaLS stub
declared `WaitOpts : HttpOpts` and so *advertised* `headers` on the polling verbs.
[[module-opts-silently-ignored]] turned that documented-but-unimplemented surface from a silent
no-op into a hard refusal, which is exactly how a gate is supposed to earn its keep — the drift
surfaced in the docs instead of in someone's proof.

<!-- claim: eval-snippet-starting-with-a-comment recorded=2026-08-14 -->
**A snippet that begins with a Lua comment can be evaluated.** `prova eval -- '<code>'` ends flag
parsing and takes what follows verbatim — the conventional spelling, and the one this CLI already
uses for `prova lock <token> -- <cmd>`. `-` (read from stdin) remains the other door.

**The refusal teaches the separator, which matters as much as having one.** The code arrives as a
single argv element, so `--` at position 0 parses as a flag; reporting `unknown flag --` is true
and useless, because the author is holding valid Lua and has just been told about flags. An
argument beginning with `--` that contains whitespace is source, not a flag — one word is what a
flag looks like — and that is enough to tell them apart and say the right thing. A typo'd `--bogus`
is still reported as a flag, which is the control that keeps the heuristic from becoming the worse
error.

It bit precisely where snippets are longest: a `[==[ … ]==]` block opening with a note about what
it does. Only the leading position ever mattered — a trailing comment was always fine.

## 26. A list verb returns a list, empty or not

## 27. A wait is bounded, and the bound is a verdict

<!-- claim: every-wait-is-bounded recorded=2026-08-15 -->
**Every primitive that blocks on a signal carries a timeout by default, and reaching it FAILS the
test rather than hanging the run.** `prova.barrier` (30s), `prova.retry` (30s), `http.wait_for`
(30s), `docker.run`'s `wait` (30s) and `docker.build`'s `first_byte` (90s) all raise, naming what
they waited for and what arrived. A default rather than a required argument — the number is a
patience, and making every caller invent one adds ceremony without adding judgement — but there is
no spelling of any of them that waits forever.

The reason is that a hang reports nothing. A suite that stops has no verdict, no line naming the
seam, and no exit code until some outer timeout kills it — CI's blunt one, or a human's. A bounded
wait converts that into a failure that says which rendezvous, how many participants arrived, and
which of the possible causes to look at first. Coordination primitives are welcome; unbounded ones
are not.

## 28. An assertion answers; it never takes the run with it

<!-- claim: equals-must-answer-not-abort recorded=2026-08-15 -->
**`equals` compares cyclic structures without walking them forever.** Identity short-circuits
first — the same table IS equal to itself, which terminates any cycle reached through it — and a
depth cap (64, far beyond real data) backstops two DISTINCT self-referencing structures by
answering "not equal" rather than recursing until the stack dies.

Before the guard this did not fail, it ABORTED: `fatal runtime error: stack overflow`, exit 134,
taking the whole run with it. Every other test lost its result, the reporter emitted nothing, and
the exit code named a signal rather than a verdict. That is categorically worse than a wrong
answer — a wrong answer is a finding, and a dead process is an absence. The same bound-and-report
doctrine as [[every-wait-is-bounded]], applied to recursion instead of time.

`:matches` was never affected: it walks the finite SHAPE the author wrote, not the subject.

**Found by paying down the coverage floor**, which is the argument for doing that as a discovery
exercise rather than a number to clear: no bug report, no failing suite, and the defect was
reachable from any proof comparing user data with a back-reference in it.

<!-- backlog: digest-identity-is-location-dependent recorded=2026-08-15 -->
**A conduct-identity digest carries each file's ABSOLUTE path, so the same content at two locations
never matches itself.** `digest_paths` contributes `emit_path(&p)` per file, which is the resolved
absolute path — two checkouts of one commit, or one repo moved, produce different digests for
identical bytes.

That is RIGHT for what it exists for: conduct identity within one run, on one machine, where the
question is "are these two conducts the same question, so one execution can answer both" — and
there the location genuinely is part of the environment. It becomes a trap the moment an identity
outlives that scope. A cached verdict keyed on this can never be shared between a developer's
checkout and CI's, or between two agents' worktrees of the same tree — which is exactly what
[[resumable-runs-incremental-verdicts]] would want, and it would fail as a permanent cache MISS
rather than as an error, so the symptom is "the cache never helps" rather than anything diagnosable.

`fs.digest` exposes the same function to suite authors, where "content-addressed" reads as a
promise about CONTENT and location-dependence is a surprise.

Pinned as the current contract by a unit test rather than changed, because the fix is a decision
about scope, not a bug: make the path RELATIVE to the identity's root (portable, shareable, and
the root becomes part of what a caller declares), or keep absolute and say so in `fs.digest`'s own
summary. Worth settling before anything persists a verdict across runs.

<!-- backlog: lock-waits-are-unbounded recorded=2026-08-15 -->
**A cross-instance lock wait has no bound, and it is the one place the rule above does not hold.**
`locks::hold` flocks blocking with no deadline, so a run queued behind another instance's `cargo`
waits indefinitely. It is safer than it looks — the kernel releases a flock when its holder dies,
so the wait is always behind a LIVE process doing real work, never a dead one — which is why this
is a backlog item and not the same severity as a hang on a signal that may never come.

What it costs is diagnosis. In CI the job stops until the runner's own timeout fires, and that
kills the process with no line naming the token, the holder, or how long it waited — the run banks
`run.lock_wait_ms` only if it finishes. Suggested shape: a generous default bound (minutes, not
seconds, since a legitimate build genuinely takes them) whose expiry fails with the token, the
elapsed time, and the holding pid if it can be read. `prova lock --machine` and the `[runner]`
provision are the two callers that would feel it first.

**Deliberately NOT in scope:** `shell.run`'s optional `timeout`. Running an arbitrary command of
unknown duration is the verb's whole job, so a default would abort legitimate work; `idle_timeout`
and `first_byte` exist as the opt-in liveness bounds for the cases where silence IS the signal.

<!-- claim: a-measurement-must-prove-it-measured recorded=2026-08-16 -->
**A number that stops being produced correctly must fail loudly, not keep reporting.** The
black-box coverage layer measures the RECURSION — the instrumented binary runs the suite and
`LLVM_PROFILE_FILE`'s `%p` rides into every `prova.bin` child, because that is where the runtime
actually executes. For four days it measured none of it, and said so with a number.

The cause was a variable read by nobody. The conduct set `PROVA_TRAMPOLINED=1` meaning "this IS
this tree's build — skip the hop", but that named a re-exec mechanism which had since been retired,
and nothing in the source ever read it. So `prova.bin` resolved to the declared `[runner]` — the
ordinary uninstrumented `target/debug/prova` — and every child contributed nothing. A 197-second
conduct wrote **2** profraws. The layer read 45.47% against a 68.99% floor, and the ratchet took
the blame: the honest-looking conclusion was that four days of feature work had diluted coverage,
because a percent arrives with no evidence of how it was obtained.

Three things now make that impossible to repeat, and they generalize past coverage:

- **The intent is executable.** `PROVA_SUBJECT_BIN` names the subject outright and is read by
  `subject_bin`, so a conduct that means "test THIS artifact" says it in a way that can be wrong
  out loud. Proved in `runner_provision_test.lua`, inheritance included — the recursion is
  arbitrarily deep, which is why it is an env var rather than a flag.
- **The measurement proves it measured.** The conduct counts `suite-*.profraw` before reporting and
  refuses below a floor far under a healthy run and far above a broken one. Two profraws is no
  longer a number, it is an error naming the cause.
- **Every layer prints its denominator, before the ratchets fire.** Covered/total, not just a
  percent — the numerator falling with a fixed denominator looks nothing like dilution, and that
  is the read that ends this argument in seconds rather than sessions.

Re-measured after the fix: blackbox 45.47% → **73.04%**, merged 79.96% → **86.37%**, unit unchanged
at 73.47%. No coverage had been lost; all three floors were being held all along. The floors were
then re-banked with bands (see `proofs/coverage/coverage_test.lua`), because the thing that made
this expensive was never the measurement alone — it was a peak-banked ratchet that made a wrong
number look like a real regression.

<!-- backlog: file-locking-is-a-no-op-on-windows recorded=2026-08-16 -->
**Two subsystems take file locks, and on Windows both compile to nothing.** `locks.rs` has said so
since it was written — the `LockFileEx` twin "lands with the Windows runner", and until then a
cross-instance lock is run-scoped there, which is the pre-lock behavior. `barrier.rs` then
hand-rolled its own `libc::flock` instead of reusing that shape, and so did not COMPILE for
windows-x86_64 at all; it now carries the same documented no-op, which is what unblocked v0.24.0's
release matrix.

What the no-op costs, per site. For `locks`, a house rule like "one cargo at a time" simply does
not bind across instances on Windows. For `barrier`, arrivals are a read-modify-write of one small
file, so two simultaneous arrivals can lose an increment — and that direction is the safe one: a
lost increment TIMES OUT, reporting fewer arrivals than came, which is loud. It cannot manufacture
a vacuous pass, because that needs an increment nobody made.

Do it as ONE piece of work across both sites, and do it where it can be run. Writing a lock
implementation blind is the unverified-claim problem with a worse blast radius than usual: a lock
that silently fails to exclude produces exactly the races it was added to prevent, and no local
test can tell you. Same prerequisite as [[windows-ut-relink-denied]] — someone on the platform.

The sharper lesson is the one about reuse: `locks.rs` had already met this problem and written the
portable shape down. A second subsystem solved it again from scratch, inherited none of the
thinking, and the gap surfaced as a broken release rather than a compile error on anyone's desk —
because the only leg that builds Windows is the release matrix.

<!-- backlog: ut-lane-cannot-see-what-cargo-test-sees recorded=2026-08-16 -->
**The `ut` lane deputes to nextest, so a whole class of test-isolation defect cannot fail locally.**
nextest runs each test in its OWN PROCESS; `cargo test` runs them as threads in one. Tests that
share state keyed on something per-process — a scratch directory named after `std::process::id()`,
a static, an env var, a fixed port — are therefore isolated under `prova run ut` and racing under
`cargo test`, and the local bar reports green either way.

Not hypothetical: `barrier.rs`'s tests keyed their home on the pid alone and wiped it on entry, so
under threads one test deleted another's barrier state mid-run. `prova run ut`, `prova run all` and
`prova run release` were all green locally; the v0.24.0 release then failed in the Release
workflow's Setup leg, which runs plain `cargo test`. The tag existed and nothing shipped. A barrier
is shared state by construction, which is what makes it the worst possible place for this blind
spot — and the tests were written the day the primitive landed, so the exposure was one release
long.

The asymmetry is the problem, not nextest: the release gate is judged by a runner the local gate
never uses. Options, in rough order of appeal — run the unit layer BOTH ways in the `release`
profile (cheap, and it is the profile that means "fit to be a version"); or align CI's Setup leg
onto `prova run ut` so one runner judges everywhere, which is the cleaner story but moves what CI
checks; or leave it and accept that this class ships. Worth settling before the next release
rather than after it.

<!-- claim: unparseable-durations-are-dropped-not-refused recorded=2026-08-16 -->
**A malformed duration at a closed boundary is silently dropped, and it produces the unbounded
wait this section forbids.** `client_opts` parses its `timeout` as
`opts.get("timeout")?.and_then(|s| parse_duration(&s))`, and `parse_duration` answers `None` for
anything it cannot read. So `http.client{ base_url = …, timeout = "5 seconds" }` — a plausible
spelling of a real grammar (`"5s"`) — configures a client with **no timeout at all**, and the
proof that meant to bound its wait waits forever instead.

This is the module-opts gate's blind spot rather than a gap in it. The gate closed the KEY set, so
`timeut = "5s"` is now refused by name; the VALUE under a correctly-spelled key is still parsed
best-effort, and the two failures look identical to an author who typed one character wrong in
either place. Found while writing unit tests for `client_opts`, not from a report — the silent path
has no symptom until something hangs.

Suggested shape: parse strictly at every boundary that takes a duration string — refuse with the
site, the key, and the accepted grammar, the same way an unknown key is refused. Worth auditing as
one sweep rather than one call site, since `and_then(parse_duration)` is the idiom wherever a
duration crosses from Lua, and every instance of it fails the same way.

<!-- claim: a-list-verb-returns-a-list recorded=2026-08-15 -->
**Anything that returns a sequence returns something that IS a sequence, including when it is
empty.** `json.decode('[]')`, `csv.decode` over a header-only file, and `fs.glob` with no matches
all hand back a table wearing the array metatable, so they re-encode as `[]` rather than `{}`.

A bare Lua table cannot say which it is: an empty list and an empty map are the same value. So
every list-returning verb had a shape-loss bug on exactly one input — the empty one — and
`json.encode(fs.glob(dir, pattern))` silently emitted `{}` the day the directory happened to have
no matches. Plenty of APIs treat `[]` and `{}` as different requests, and the failure lands at the
far end of one, naming nothing about the glob. That the empty case is also the least-exercised
while writing a proof is what makes it worth a claim rather than a footnote.

The marker is inert: `#`, `pairs`, `ipairs` and indexing are unchanged, and `values_equal` and
`subset_mismatch` compare structure rather than metatables, so a decoded list still `equals` a
plain literal and still `matches` a shape. It is consulted only on the way back out through an
encoder. `modules::list_table` is the one constructor; new list verbs use it rather than
`create_sequence_from`.

**Found by paying down unit coverage**, which is the argument for that exercise: the first test
written against the JSON boundary was going to *document* the asymmetry as a known quirk. It is a
bug, and it generalized to two more verbs the moment it was named.

<!-- claim: url-encode-had-no-inverse recorded=2026-08-15 -->
**`url.decode` is `url.encode`'s exact inverse, and both are COMPONENT codecs.** A space is `%20`
and `+` is a literal plus — form encoding disagrees on exactly that character, so a decoder
borrowed from the wrong convention corrupts every value containing one, silently, and only for the
inputs that happen to include it. Decoding yields BYTES, because a percent sequence can carry any
octet and turning an invalid one into U+FFFD is the corruption [[http-binary-response-corrupted]]
was fixed for, one namespace over.

`encode` shipped without a decode half. A proof that RECEIVED a percent-encoded value — a
redirect's Location, a query parameter, a header — had nothing to read it with but a hand-rolled
decoder, which is exactly the "well-known place to introduce a quiet bug" this module declares a
crate to avoid. The missing verb pushed authors into writing the bug themselves.

<!-- claim: csv-encode-loses-no-column recorded=2026-08-15 -->
**`csv.encode` refuses a row carrying a column the header row lacks, and writes nothing when there
is nothing to describe.** Headers are taken from the FIRST row, so a later row's extra field
vanished with a successful return — the column was in the data and not in the output, and which row
happened to be first decided what was lost. Declaring `headers` is different: naming the columns IS
choosing them, so a deliberate projection stays legal. A row MISSING a column is not loss either —
that is an empty cell.

Encoding zero rows used to emit `""` — a header line declaring one column whose name is the empty
string, which decodes back to a row shape nobody asked for. Nothing to describe means nothing to
write.

<!-- claim: csv-duplicate-headers-refused recorded=2026-08-15 -->
**`csv.decode` refuses duplicate headers rather than dropping a column.** Rows are header-keyed
maps, so two columns of one name cannot both survive — the second overwrote the first and the lost
column was never mentioned, which is data loss wearing a successful return. The contract is
unsatisfiable for that input, so the verb says so and names the header, rather than answering with
a row that is quietly missing a field.

<!-- backlog: unit-coverage-diluted-since-the-arc-closed recorded=2026-08-15 -->
**Unit coverage has fallen from its banked floor because feature work outran unit-test writing —
the ratchet is right, and recovering it is a campaign rather than a patch.** Measured 2026-08-15:
`rust.coverage.unit` 72.296% (20316/28101 lines) against a floor of 73.460%, reproducing CI's
72.259% exactly. The merged number is 79.05% against 84.69%.

**Why.** The floor was banked 2026-08-11 as the CLOSING ACT of the north-star coverage arc — the
high-water mark of a push whose whole purpose was raising it. The four days after landed conduct
leases, selection pushdown into deputies, `--covering`, the timing capability, lock-wait narration,
conduct identity, start budgets, and this session's six items. Every one adds shipping Rust proven
by black-box proofs, so the denominator grew faster than the numerator. CI went red within a day of
the floor being set, which fits exactly. Nothing is broken; the gate is reporting the thing it
exists to report.

**The gap is ~327 covered lines.** For scale: 22 genuine unit tests written this session (the opts
gate's ordering and suggestion rules, the tar builder's parent entries and modes, http's header
and URL joining, the tempdir label sanitizer, eval's argument parser) moved the number **+0.04pp**.
Recovering 1.16pp by hand is roughly twenty-five more batches of that size — the same shape as the
arc that reached 73.46 in the first place.

**Ranked worklist, by uncovered lines at the unit layer:** `grpc_mock.rs` 386 (52.1%),
`mcp/blocking.rs` 381 (35.8%), `terminal.rs` 378 (49.1%), `cmd_attest.rs` 370 (45.9%),
`broker.rs` 368 (29.5%), `socket.rs` 298 (72.8%), `matchers.rs` 284 (70.8%), `modules.rs` 260
(46.6%), `dispatch.rs` 247 (73.4%), `formats.rs` 242 (33.0%). Any two of the top five closes it.

**What NOT to do.** Do not write tests to move the number — a suite that exercises lines without
asserting behavior is the exact vacuous-proof disease the rest of this document is about, and it
would be worse than a red gate because it would be a green one that means nothing. Do not lower the
floor casually either: it was earned, and the first day it becomes inconvenient is the worst moment
to give it up. Excluding `xtask` from the denominator (88 lines, 0%, build automation rather than
shipped code) is defensible on its own merits and worth +0.23pp, but it is a scope decision to make
deliberately, not a way to get to green.

<!-- backlog: overlap-assertions-depend-on-host-headroom recorded=2026-08-15 -->
**A scheduling proof that asserts two units OVERLAP is only true when the host has room to overlap
them — and it is now the release gate's flakiest assertion.** `prova-core::resources
modes_are_independent_of_how_the_token_was_made` runs a scenario at `-j 8` where two READERS of one
token each `record("enter")`, `prova.sleep(40)`, `record("exit")`, and asserts the pairs interleave.

**The mechanism, now that it has been seen twice:** overlap has to happen inside a 40ms window. On
a loaded box the scheduler may simply not start the second reader before the first finishes
sleeping, so the events do not interleave and the assertion fails — with nothing wrong. Observed
2026-08-15 in two separate `prova run all` sweeps, both times passing in isolation, in a standalone
`cargo nextest` run, and in the other sweeps the same night.

**Only the reader half is fragile.** The writer half asserts units do NOT overlap, and load makes
that MORE likely to hold, so it cannot fail this way. Serialization is also the property that
actually matters; concurrency is the one being measured by stopwatch.

**Priority is no longer low.** `prova run release` gates releases now, and a gate that fails
randomly is the thing that trains people to re-run rather than read — the exact failure the
switched-off line already demonstrated this session.

**Shape worth building:** make the readers RENDEZVOUS instead of racing a clock — each records its
entry, then waits (generously) to observe the other's entry before exiting. Concurrency then proves
itself by construction: if the scheduler runs them together both proceed immediately, and if it
genuinely serializes them the first blocks until timeout and fails for the right reason, naming
scheduling rather than timing. Do NOT widen the sleep (slower for everyone, same race), retry
(hides a real serialization break), or drop the assertion (gives up the claim that `reads` is
concurrent at all).

<!-- backlog: module-opts-gate-remaining-namespaces recorded=2026-08-14 -->
**Every CONSTRUCTOR is closed; the builder and filter surfaces are not.** `crate::opts::Closed` now
gates nineteen entry points: `shell.run`/`spawn`, `docker.build`/`run` and its nested `wait`, the
`http` request/client/`wait_for` options, `graphql.client`, `http.mock`/`proxy`,
`grpc.mock`/`proxy`, `socket.mock`/`listen`/`proxy`, `websocket.mock`/`proxy`,
`terminal.mock`/`proxy`, and `shell.proxy`. Those are the tables an author writes by hand, where a
typo silently mis-provisions.

Still open, and deliberately: the **stub builders** (`:on{…}:reply{…}`, `route`, `respond`) and the
**journal filters** (`received{…}`). Those are a different kind of table — a filter is a structural
subset match over arbitrary recorded fields, so its key set is the SUT's vocabulary rather than
prova's, and closing it would refuse legitimate matches. Any gate there has to distinguish prova's
own keys from the payload's, which is design work, not enumeration. `wiretap`, `measure` and the
`formats`/`junit`/`sarif` readers remain unexamined.

Two things the sweep turned up worth keeping: `websocket.mock` read its opts argument NOT AT ALL,
so every key ever passed to it was dropped whole — its accepted set is empty, which is now said out
loud. And several constructors read part of their set through a helper (`socket.proxy` takes four
keys via `proxy_config`), so a closed set derived from one function's own `get` calls would have
refused options that work — worth remembering when the remaining surfaces are done. `sql` needs
nothing: `sql.client(url)` takes a string, so there is no table to close.

<!-- backlog: windows-ut-relink-denied recorded=2026-08-13 -->
On windows-latest CI, 'prova run ut' fails with 'failed to remove file target\\debug\\prova.exe — Access is denied (os error 5)': the conductor IS the file the deputy's cargo build wants to relink, and Windows refuses to replace a running executable. Long-standing (every Build run on main for at least a day). The unix runners are unaffected because a running binary can be unlinked. Candidate fixes: have CI conduct the ut lane through a COPY of the binary (copy target/debug/prova.exe to a scratch path and invoke that), or teach the deputy recipe to exclude the conducting binary's own target on Windows. Notably the RELEASE gate passes on Windows because it builds rather than conducting a rebuild under itself.

<!-- claim: local-clippy-weaker-than-ci recorded=2026-08-13 -->
**One toolchain lints this tree, everywhere.** `rust-toolchain.toml` pins an exact version with
`clippy` and `rustfmt`; rustup honors it for every `cargo` invocation under the root, and GitHub's
runners ship rustup, so CI obeys it with no workflow change. The quality lane also logs the clippy
version it ran, so a future divergence shows up in the output rather than on main.

An exact version rather than `stable`, deliberately: `stable` reintroduces the same failure on a
six-week timer, since a newly-released clippy can fail a tree that passed yesterday with nothing in
the diff to explain it. Bumping the pin is how a lint change enters — in a commit that can be
reviewed and reverted.

The divergence this closes: the tree defaulted to whatever rustup had (a nightly-1.95 from January)
while CI used the runner's *current* stable, whose `question_mark` lint failed
`crates/prova-cli/src/mcp.rs:97` under `-D warnings` while the local run said nothing. That is the
worst direction for a gate to be wrong in — `prova run quality` is what an agent trusts before
pushing, and it UNDER-reported. (The offending code has since been rewritten as a `match`, so the
original finding is no longer reproducible; what is verified here is the mechanism — the pin is
honored, the tree builds on it, and the gate is clean under it.)

## 17. A topology that takes minutes cannot be inhabited — `prova start`'s budget is fixed

<!-- claim: start-timeout-is-unconfigurable recorded=2026-08-13 -->
**A topology declares how long it needs to come up; the invocation may override it.** `startup =
"15m"` on a `[topologies.<name>]` entry is the definition's own statement of its cost — the same
principle the manifest already applies to everything else it declares — and `prova start
--timeout 20m` is the ad-hoc override for the machine having a bad day. Precedence is flag,
then declaration, then a 300s default; the error names the budget that fired and both ways to
change it, because a fixed limit whose only symptom is "did not come up" teaches nothing.

Without it, a topology whose honest startup exceeds five minutes — a kind cluster with an ingress
controller, six image side-loads and eight rollouts — cannot be *inhabited* at all: the same
factory a suite fixture builds happily can never be `prova start`ed. That costs exactly the verb
the inhabited/fixture pair exists to provide, and it bites smaller stacks the first time a source
edit makes an image rebuild.

<!-- claim: start-timeout-orphans-containers recorded=2026-08-13 -->
**A `start` that gives up tears down what it began.** The budget's expiry signals the holder
(SIGTERM — the same signal `prova down` sends, which runs the identical in-process teardown) and
waits for it to release, escalating only if it will not; it never SIGKILLs a holder that is
holding containers. A killed holder runs no teardown, so its containers survive, and the *next*
attempt fails on a host port the orphans still hold — reported as a port conflict, which is the
previous failure's residue wearing an unrelated diagnosis. This is
`verifiers.md#timeout-reaps-the-conduct` at the topology's scale: dead means the tree is dead,
and the cure must never be `docker ps -q | xargs docker rm -f` typed by a user who should not
need to know it.

## 18. A request body is named exactly once, in whichever shape the endpoint wants

<!-- claim: http-form-and-raw-bodies recorded=2026-08-13 -->
**`json`, `form` and `body` are three spellings of one thing, and a call may use exactly one.**
`form = { grant_type = "password", … }` sends `application/x-www-form-urlencoded`;
`body = "…"` sends bytes verbatim; `content_type = "…"` names the media type for either, and wins
over the type the shape implies. Passing two is refused rather than ranked: an `if json … else if
body` chain sends a request the author did not write and then reports the endpoint's honest answer
to it, so the debugging starts at the server.

Before this, `HttpOpts` was `{headers, json, timeout}` and OAuth 2.0 token endpoints require
form encoding — so the two proofs that obtain a real token (docker and kubernetes topologies alike)
called `curl` through `shell.run`, putting a host-tool `requires` on a proof whose subject is HTTP.
The encoding is `form_urlencoded`'s rather than hand-rolled, because a body that mis-escapes `+`,
`&` or UTF-8 fails at the far end of an exchange whose error names none of that.

## 19. A redirect can be observed instead of followed

<!-- claim: http-redirect-control recorded=2026-08-13 -->
**`redirects = false` returns the 3xx itself; `redirects = N` caps the chain; the default still
follows.** `status` and `headers.location` are intact either way, which is the whole assertion:
"an unauthenticated visitor is redirected to `/auth/login`" is a statement about the hop, not about
its destination.

One key rather than a `redirects`/`max_redirects` pair — they would be two spellings of one
question, and a table carrying both would need a precedence rule nobody could guess. Unconditional
following made auth flows unprovable: the client had already taken the 307 and returned whatever
lay beyond it (a 500, in the case at hand, because the identity provider was deliberately absent),
so a working gate read as a broken app.

## 20. Kubernetes topologies are hand-rolled kubectl

<!-- backlog: kubernetes-topology-support recorded=2026-08-13 -->
**Cluster-shaped topologies deserve the same first-class treatment containers have.** Standing a
stack up in kind is roughly two hundred lines of `shell.run{"kubectl", …}` in a factory: apply,
`rollout status`, `wait --for=condition`, image side-load, port-forward. Three sharp edges recur
and every author will meet them: `kubectl wait` refuses outright when no pod matches yet, so
readiness must be sequenced rollout-then-ready; `kind load docker-image` fails on multi-arch
images in Docker's containerd store ("content digest not found"), so images must travel as
single-platform archives; and a port-forward is a long-lived process whose readiness races the
first client, which the author must retry by hand. A `k8s` namespace could own all three, with
**port-forward as a managed resource** (`k8s.port_forward(ctx, {service = …, port = …})`
returning the usual `{url, host, port}` and tying teardown to the scope) as the highest-value
piece — it is the seam every host-side proof needs against an in-cluster service.

## 21. A containerized recipe cannot mount a file

<!-- claim: containerized-mounts recorded=2026-08-13 -->
**A container's configuration is carried in, not baked.** `docker.run{ files = { ["/abs/path"] =
{ text|file|dir = …, mode? = "0755" } } }` streams content into a CREATED but not-yet-started
container, so the process sees it at boot and a one-line config change costs nothing. Recipes take
the same key, and a caller's entries win over the recipe's by path — which is what ends "fork the
Dockerfile to change one line".

**Not a bind mount, deliberately, and this is the whole design.** `binds` is one defaulted field
away in bollard's `HostConfig`, and it names a path on the DAEMON's filesystem. `docker.run` talks
to whatever `DOCKER_HOST` resolves to, so against a remote or rootless daemon a scope tempdir is
simply not there — and Docker's classic answer to a missing bind source is to create an EMPTY
DIRECTORY. The container boots, finds no realm, and fails later as an auth error naming nothing
about mounts: a silent wrong wearing a configured face, which is the failure class
[[module-opts-silently-ignored]] and [[context-tempdir-not-idempotent]] were each paid to remove.
Content injection travels the same API as every other call, so it works wherever the daemon does.

**The entry shape is `docker.build`'s `secrets`, on purpose.** One of `text`/`file`/`dir`, and a
bare string refused — ambiguous between a literal and a path, where guessing wrong either writes
the path as content or reads a file nobody named. Everything checkable is checked before the daemon
is touched: absolute container paths, exactly one source, a source that exists.

**The immutability was never deliberate — it was the only channel available.** What is worth
keeping is REPRODUCIBILITY, and that survives intact: the bytes come from the proof, deterministically,
every run. The remaining reason to bake is cost, not principle — a cached layer beats a per-container
upload for content that is large or expensive to produce, and `prova learn doubles` says so.

A real bind (live host coupling for a `prova up --watch` dev loop) is a different feature with
different honest caveats and is deliberately NOT shipped here; adding it alongside would make
`files` read as the cautious option rather than the correct one.

## 22. A response body crosses into Lua as bytes

<!-- claim: http-binary-response-corrupted recorded=2026-08-14 -->
**`res.body` is byte-exact for any payload, and `res:save(path)` puts it on disk without it ever
becoming a Lua value.** The response is held as bytes end to end — `reqwest`'s `bytes()`, never
`text()` — so a zip arrives the length its server declared and opens in a foreign reader.

The fix needed no new accessor to be *correct*, which is worth stating because the obvious API was
the wrong one: **a Lua string is a byte string**, so handing Lua the raw bytes makes `res.body`
exact for binary and identical to before for text. A separate `res.bytes` would have been a second
spelling of a now-correct thing. `save` earns its place for a different reason — `fs.write` takes a
UTF-8 `String` and would reject those very bytes — so "I need the artifact on disk" should never
round-trip through Lua at all. Refusing non-text content types, the other candidate shape, would
have been a breaking answer to a problem that no longer exists.

What made this worth a proof rather than a note is how well it hid. `text()` is a LOSSY UTF-8
conversion: each invalid byte becomes U+FFFD, three bytes out for one byte in, so a 22181-byte zip
came back as 34220 unusable ones — inflated, not truncated. Every cheap check still passed. Status
200, `#body` plausibly large, `body:sub(1, 2)` still `"PK"` because ASCII survives untouched. A
proof that sniffed the magic number asserted nothing about the payload and reported green, and a
suite can carry that false confidence indefinitely. Found while proving that a rendered project's
archive downloads — the download proof had to shell out to `curl`, so a proof about HTTP once again
required a host tool (see [[http-form-and-raw-bodies]]: same surface, opposite direction).

## 23. A scratch directory is addressed, never manufactured

<!-- claim: context-tempdir-not-idempotent recorded=2026-08-14 -->
**`ctx:tempdir(name?)` answers with a directory belonging to this scope instance — the same one
for the same name, forever.** `ctx:tempdir()` is the unnamed one; `ctx:tempdir("plugin")` and
`ctx:tempdir("consumer")` are two more. All are removed when the scope ends, and the name is
embedded in the directory's own path. There is no arity at which the verb manufactures something
new, so no call can surprise its second caller.

It used to be a FACTORY, handing back a fresh directory per invocation, so `fs.write(t:tempdir() ..
"/cookies.txt", …)` followed by `fs.read(t:tempdir() .. "/cookies.txt")` wrote one directory and
read another. Nothing errored — reading a missing path in a fresh directory simply yields nothing —
and the proof failed much later on whatever consumed the result. In the case at hand that was a
curl cookie jar, so a login flow appeared to be rejected by the identity provider when the session
cookie had merely been written somewhere the next step never looked. About an hour, chasing an auth
bug that did not exist.

**The name was the defect; the missing primitive was the reason it survived.** `ctx:tempdir()`
addresses the scope, the way `ctx:use`, `ctx:defer` and `ctx:log` all do, so it reads as an
accessor and behaved as a factory. But plain memoization is only half an answer: fifteen call sites
in this repo genuinely needed SEVERAL scratch directories, and the only verb that gave them was
`fs.tempdir()`, which is unmanaged — so each of them hand-rolled a counter and a subdirectory. That
repetition is what a missing primitive looks like. Keying the accessor serves both needs with one
verb and keeps idempotence at every arity; the fifteen counters are gone.

**The name on disk is not decoration.** The hour this cost was spent asking which directory the run
had actually written to. Three sandboxes under indistinguishable hex names leave that question to
be re-derived from the proof; `…-plugin` and `…-consumer` answer it with `ls`. Names are sanitized
to `[A-Za-z0-9._-]` — they arrive from Lua and land in a path, where a `/` would silently nest the
directory somewhere else.

Memoization is per scope INSTANCE, not per run — a fixture and the test using it are different
instances and must not share, or a file-scoped directory would leak one test's scratch files into
the next. `fs.tempdir()` remains only as the unmanaged escape hatch for code with no context to
ask.

# Round six — 2026-08-16 (an upgrade landing mid-session in a consumer repo)

## 29. A semantic change to a verb that still compiles has no landing signal

<!-- backlog: tempdir-migration-was-silent recorded=2026-08-16 -->
**[[context-tempdir-not-idempotent]] was the right fix, and for code written before it the failure
mode inverted rather than disappeared — silently, at the same call sites, with no signal at any
layer.** The factory→accessor change means a pre-existing proof that called `ctx:tempdir()` three
times for three roles now gets one directory three times. Nothing errors: three names bind, three
directories exist as far as the proof can tell, and the roles collapse into one.

That collapse is worse than the bug it replaced, in one specific way. The old failure SEPARATED
things that should have been one, and its symptom was an absence — a file that was not there. The
new one MERGES things that should have been distinct, and its symptom is a false equality: a proof
whose subject is "these two locations differ" now compares one location with itself and passes, or
takes the branch it exists to forbid.

Found the hard way, in the aegis repo, about forty minutes after the upgrade landed mid-session:

* `install_test` §5 hands a lifecycle verb a state dir that is NOT the installed service's and
  asserts it refuses. Its three roles — plist dir, installed state dir, and the other one — became
  one directory, so `installed == other`, the guard saw a match and waved the command through, and
  the proof drove `launchctl bootstrap` under a constant label instead of asserting a refusal.
  **That guard exists because the same accident once took down the author's daily driver.** The
  upgrade quietly converted a safety proof into the act it forbids; nothing was harmed only because
  the label happened to be loaded already.
* `mcp_proxy` §7 asserts that registering an MCP server with no daemon running is refused. Its
  "orphan" state dir became the LIVE daemon's, so the section was registering with one and
  asserting on the answer.

Both had passed thirteen minutes earlier. The reds arrived with nothing in the consumer repo having
touched them, so the diagnosis began — correctly, and expensively — by restoring `crates/` from the
parent commit to rule the local change out. The answer was in `ls -la $(which prova)`.

**The honest difficulty, and why this is backlog rather than a claim: there is no clean runtime
guard available.** Calling `ctx:tempdir()` twice is the PRIMARY intended usage — `fs.write(t:tempdir()
.. "/x")` then `fs.read(t:tempdir() .. "/x")` is the exact pattern the fix exists to make work — so
erroring on a second unnamed call would break the thing it repaired. The stale shape and the correct
shape are identical at runtime; only the author's intent separates them.

So the options are not runtime ones. A migration lint that flags a unit binding two unnamed
`tempdir()` results to two different locals would catch the stale shape precisely, and its false
positives are cases where naming is harmless anyway. Cheaper, and possibly enough: a CHANGELOG entry
saying "if you called this more than once for more than one directory, name them" — a consumer who
knows to look can grep. What cost the time here was not the missing guard. It was that a behavioral
change to a verb arrived indistinguishable from no change at all, in a tool consumers upgrade by
reinstalling.

## 30. A repeated flag is not a second value; it is a discarded one

<!-- backlog: repeated-update-baseline-silently-drops recorded=2026-08-16 -->
**`--update-baseline=<name>` keeps only its LAST occurrence, so passing it twice banks one metric and
silently drops the other.** `dispatch.rs` assigns rather than accumulates — each
`--update-baseline=` match overwrites `self.update_baseline` with a fresh `BankSelection::Named(…)`
— while the flag's own error text advertises "metric name(s)", so the comma form
(`--update-baseline=a,b`) is the supported spelling and the repeated form reads as equally plausible.

The symptom is that the run succeeds for one metric and, for the other, prints the very message
telling you to do the thing you just did:

```
prova: baseline tightened quality:rust.expect.production 85 -> 84
prova: baseline held quality:rust.duplication.clones stays at 7
       (measured 6; no goal — bank it by name: --update-baseline=rust.duplication.clones)
```

At a glance that reads as prova declining to move a ratchet, not as prova discarding an argument.
Found while banking two improved ratchets in the minion repo; running it again with one flag at a
time worked, which is what made the cause obvious in hindsight.

This is [[unknown-test-opts-silently-ignored]]'s principle — "a dropped option is worse than a
rejected one — it reads as configured" — unhonored one layer out, at argv rather than at a Lua opts
table. The line the DSL and the manifest both hold has not been walked across the CLI's own
repeated-flag handling, and there is little reason to think this is the only flag that assigns where
a reader would expect it to accumulate.

Either resolution beats silence: union the occurrences (what a repeatable flag conventionally means,
and what the comma form already expresses), or refuse the second naming both selections. Worth
auditing as one sweep over `dispatch.rs` rather than one flag, since the shape — `self.x = Some(…)`
inside an arg loop — is the idiom wherever an option takes a value.

<!-- backlog: coverage-lane-blocked-by-a-contended-timing-proof recorded=2026-08-19 -->
**The coverage lane cannot complete, and not because of coverage: one timing proof measures a
hundred times slower inside the instrumented suite than alone.** `proofs/shell/lease_test.lua`'s
Ctrl-C proof — "an interrupted prova takes its conducts with it" — spawns a prova, SIGINTs it, and
waits for the reaper to sweep the leased conduct. The conduct gates on a green suite before it
measures anything, so this one red stops every layer from reporting.

Measured 2026-08-19, all under the instrumented binary:

| how it ran | result |
|---|---|
| the proof alone (`prova proofs/shell/lease_test.lua`) | **PASS in 268ms** |
| inside the full instrumented suite, 5s sweep bound | FAIL at ~8s |
| …bound widened to 30s | FAIL at 44.8s |
| …plus `serial = true` and a 15s bound | FAIL at 23.5s |

Same binary, same code, a hundredfold apart — so it is contention, not instrumentation. But
**widening the bound and serializing the unit both failed to fix it**, which rules out the two
obvious readings: it is not simply "the bound was too tight", and it is not competition from other
units in the same process (`serial` is process-wide, and the conduct runs a NESTED prova whose own
`--jobs` workers are outside that guarantee).

Both attempted fixes were reverted rather than shipped: a change that does not fix the failure it
was written for is noise in the diff, however plausible it reads.

Correlation worth checking first: the lane completed on this tree before
`proofs/spec/durations/` landed and has not since — those proofs spawn eight instrumented
`prova.bin eval` children. That points at total process pressure in the nested run rather than at
any one unit. Whether the sweep is genuinely not happening or merely late is the first thing to
establish, and the proof cannot currently tell those apart — it reports "did not sweep in N
seconds" for both.

**Update 2026-08-20: intermittent, not deterministic — and no longer blocking.** The full
instrumented suite ran green the next day (727 passed, the Ctrl-C proof at 273.9ms) with no change
to the proof, on a machine that had just had two runaway background pollers killed. Three
consecutive failures then, four consecutive passes now. That reclassifies it: not "fails under
instrumentation", but "fails under enough contention", where the threshold is somewhere between an
idle laptop and whatever those pollers were adding.

So it no longer blocks [[coverage-denominator-is-not-reproducible]] — that one is resolved, with the
lane green. What remains is a proof that measures wall-clock and will fail again on a busy CI box,
reporting "the reaper did not sweep" when it means "nobody scheduled me". Two attempted fixes
(widening the bound to 30s, `serial = true`) were tried and REVERTED because they did not work; a
third that has not been tried is to make the proof distinguish "did not sweep" from "was not
scheduled" — the message cannot currently tell them apart, which is why an afternoon went into
reading it as the former.

<!-- claim: coverage-denominator-is-not-reproducible recorded=2026-08-19 -->
**The coverage ratchet is measuring an unstable denominator, so its floors cannot be re-derived —
including at the commit that banked them.** Four clean-slate conducts, with
`target/llvm-cov-target`, `target/exec-stage` and `target/suite-profraws` wiped before each:

| tree | merged | unit | black-box | lines counted |
|---|---|---|---|---|
| `cf5719` — the commit that BANKED the floors | **65.74%** | 59.28% | 51.09% | 35,514 |
| `main` (d78f8) | 80.99% | 73.08% | 54.54% | 29,016 |
| main + the stdio transport | 80.96% | 72.01% | 56.76% | 29,824 |
| *(the floors that commit wrote)* | *86.37%* | *73.47%* | *73.04%* | — |

The banking commit measures **twenty points below the floor it wrote**, and counts 5,690 MORE lines
than a tree three days newer whose source is strictly larger. The source did not shrink. The
instrument counts a different set of regions per build, and the percentage rides on it.

That makes conclusions unsafe in both directions: the 86.37 floor was never reachable, a later
run's "regression" is not one, and a re-bank taken today would enshrine whatever the denominator
happened to be this afternoon. **The move is NOT to re-bank.** `--update-baseline` refusing to
loosen is the guard working correctly; the value it protects is the problem.

Not a new failure mode, which is the worrying part. `coverage_test.lua` already carries a
stale-generation guard (wipe `COV_DIR` when the version stamp moves), a recursion guard (≥20 suite
profraws — 795 were produced, so the black-box layer measured the whole suite honestly), and a
comment recording a previous instance: "the 0.19.0 bump left 0.18.0 objects behind and both layers
regressed by the same ~27% — a denominator artifact, not lost coverage". Every guard closed the hole
it was built for. The denominator moved anyway, on a clean slate, with no version change.

Where to look: `cargo llvm-cov report` derives its denominator from every instrumented object under
the target dir, which is why `stage_execs` exists to move nextest's binaries out before the
black-box layer reports. That staging controls WHICH objects are scanned; it does not control how
many regions a given build emits. Codegen-unit count, incremental state and dead-code stripping all
change the region count for identical source, and nothing pins any of them — the stamp keys on
`prova.version`, which does not move when the source does. **A denominator that is not a function of
the source cannot carry a ratchet.**

**RESOLVED 2026-08-20.** The denominator was not drifting at random — the black-box layer was being
measured against a *different* basis from the other two, and which one it got depended on whether
the exec staging had actually cleared `deps/` before the report. Two regimes: ~25,300 lines when
bare, ~29,800 when the test executables were still in the scan, reading 73.6% and 62.5% for
identical code. Its numerator never moved between them (18,623 covered either way) — only what it
was divided by. So "merged" was unioning layers measured against different denominators, which is
not a number that means anything.

Three parts to the fix, and the first is the actual repair:

1. **The staging is checked on its RESULT, not its action.** `stage()` had always returned a moved
   count its caller checked; `stage_execs` never got that twin, so it silently moved nothing when
   `deps/` was absent or already bare and nothing downstream noticed. The check is on state rather
   than action because moving nothing is correct on the first conduct after a wipe and wrong on
   every conduct after one — only the state tells those apart. All three layers now report against
   one basis.
2. **The basis is banked beside what it measured**, and checked before each ratchet, so drift
   reports as "your instrument changed, by this much" instead of a coverage collapse.
3. **The floors are re-banked against that single basis**: unit 71.9, black-box 62.4, merged 80.9,
   measured twice within 0.04 of each other at 29,814 lines both times.

The black-box floor falling from 73.04 to 62.4 is not lost coverage — it is the same numerator
divided by the honest denominator. The old floor was banked in the partial-scan regime, which is
why it was never reachable from a clean tree.

Mutation-tested: banking the old 25,302 regime makes the guard fail naming +17.8% drift.

<!-- claim: stdio-cannot-drive-a-conversational-sut recorded=2026-08-18 -->
**A spawned process cannot be driven: `Process` has no stdin, so a request/response SUT is
unprovable without a co-process written in something else.** `Process` exposes `output()`,
`running()`, `stop()`, `wait()`. `shell.run{stdin=…}` and `Container:run{stdin=…}` take one
string, written before the program runs. The shim/cassette facility journals the stdin of
commands the SUT execs, which is interposition, not driving. So there is no way to write to a
live process and read the reply before deciding what to write next.

That rules out every SUT whose protocol is a conversation over stdio: MCP servers, LSP servers,
REPLs, debug adapters, interactive CLIs. It is not a niche shape — prova ships an `mcp` mode of
its own, and MCP-over-stdio is how agents reach most tools now.

Batching the requests instead is not a workaround, it is a race. Feeding an MCP server its whole
session on stdin at once (init, `render`, `respond`) made the server dispatch the tool calls
concurrently: `respond` reached the session lock before `render` had stored anything and answered
"No active render session." The proof was red for a reason that had nothing to do with the
behavior under test, and it would have been flaky rather than red had the scheduling gone the
other way.

Found proving that archetect's MCP session carries a UI breadcrumb across turns — the assertion
is specifically that state opened on turn one survives to turn two, which is unreachable in one
batch by construction. Worked around with a Python co-process driver (`subprocess.Popen`, write,
read until the matching id, write again) invoked via `shell.run`. It works and it is honest, but
it means the proof is written in Python rather than in prova, and every project with a stdio SUT
will write that same driver again.

Shape, if it earns a place: `Process:write(str)` plus a bounded read — `Process:read_line(opts)`
or an expect-style `Process:await(pattern, {timeout})` — so the exchange is a loop in the proof.
The bounded read is the load-bearing half; an unbounded one turns a wedged SUT into a hung suite,
which is the failure mode `first_byte`/`idle_timeout` already exist to prevent for `shell.run`.

## 31. `status` is the one MCP verb that cannot be aimed at a package

<!-- claim: mcp-status-cannot-be-aimed-at-a-package recorded=2026-08-20 -->
**`status` takes a `package` like every sibling verb, reports BOTH holders, and names the packages
it consulted — so `held: []` can never be read as "nothing is up" when the truth is "I could not
look".** Every other discovery verb on the MCP surface — `run`, `tests`, `switches`, `learn`,
`specs` — takes a `package`, so a server started anywhere can answer for any package on disk.
`status` took nothing. A server started outside a package (the common case for an agent harness,
whose MCP config lives per-user, not per-repo) answered `{"held": []}` unconditionally — while the
topology in question was demonstrably held: the CLI in the package directory attached to it warm
with `--topology`, and `prova ps` listed it.

The cost was a wrong answer rather than an error: "nothing is held" reads as "safe to provision /
nothing to attach to", when the truth was "you are asking from the wrong room". An agent that
trusts it cold-provisions a topology that is already up (minutes, plus a port collision on
anything with fixed host ports) or reports to its human that the demo environment is down.

**The fix is one verb answering for two holders, which are not symmetric.** A *warm* hold is the
server's own `up`, live in its memory, and it always lists — `down { name }` is package-blind, so
letting a package filter hide a warm hold would recreate the same wrong answer in mirror image. A
*detached* hold is a terminal `prova up` or a `prova start`, and its held-ness is a
`running/<name>.json` record on disk. Each entry says which (`holder: "server" | "detached"`) and
names its package, so the answer also says how to reach and reap it.

**Correcting this item as first drafted: that on-disk state is package-scoped, not machine-scoped.**
The record lives at `<home>/.prova/var/running/<name>.json` — under the package it belongs to (see
`var`/`runstate`). Any process on the machine can *read* it, which is what makes cross-instance
attach work and is what the first draft was reaching for; but nothing can *find* it without being
told a package. So "report the machine's held set regardless of package" — the shape this item
preferred, and still the shape the question means — is not reachable by aiming a parameter. It
needs a machine-wide index that does not exist yet, captured separately as
[[machine-wide-held-topology-index]]. What ships here is the reachable half: the consulted set is
the package the call names (defaulting to the server's startup affinity) plus every package this
server already holds something for, and `packages` in the result names it. An empty `packages` is
the wrong-room case, stated in a `note` rather than implied by an empty list.

Found live: a held `ybor-studio-k8s` (terminal `prova up`) that `status` could not see from an
MCP server started in the user's home; the warm `--topology` attach from the package directory
worked in the same minute.

## 32. "What is up on this machine?" is not a per-package question

<!-- backlog: machine-wide-held-topology-index recorded=2026-08-20 -->
**"What is up on this machine?" has no answer that does not name a package.** Held-topology
run-state is package-scoped on disk (`<home>/.prova/var/running/<name>.json`), so `prova ps` and MCP
`status` can only report packages they are pointed at — see
[[mcp-status-cannot-be-aimed-at-a-package]], which closed the aiming half and left this one open. An
agent that has just been handed a repo, or a human wondering what is holding port 5432, is asking
the machine-wide question, and today the only answer is to visit every package they can think of.

The shape is a machine-wide index a holder registers into alongside its package record — an XDG
state-dir file per live holder, naming its package and topology, reaped on the same clean-teardown
path that removes the package record. Then `prova ps --all` and `status` with no package are real
answers rather than scoped ones.

What makes it more than a one-liner is the second source of truth: two places recording one hold
means they can disagree, so the package record must stay authoritative and the index must be
treated as a hint that is re-verified (pid liveness, and the package record still existing) before
anything is reported as held. Cross-user visibility is the other open question — a hold in another
user's tree is real contention for a fixed host port, and reporting it means reading state prova
does not own.

## 33. One verb, two double-provision guards, and only one of them looks on disk

<!-- claim: mcp-up-does-not-see-a-detached-hold recorded=2026-08-20 -->
**`up` refuses a topology that is already up whichever holder has it — its own warm registry OR a
live record on disk — and the refusal teaches the two exits that exist.** MCP `up` used to guard
only against `warm.contains_key(&name)`, while the CLI's `up` (`cmd_topo.rs`) also reads
`runstate::read(&home, &name)` and exits 2 with `already up (pid N)`. So standing up a name a
terminal `prova up`/`prova start` was already holding provisioned a SECOND instance of the same
topology — the exact cost [[mcp-status-cannot-be-aimed-at-a-package]] names (minutes of
provisioning, plus a port collision for anything on fixed host ports), arriving through the verb
rather than through the query, and silent because neither holder could see the other.

**A refusal, not a takeover, and that is what needs teaching.** The detached holder is not this
server's to reap — `down` here would be reaping something it never provisioned — so the message
names the two real exits instead: `prova down <name>` in that package, or `prova --topology <name>`
to run against the live instance. It also names the holder's pid and package, because "already up"
without saying *where* sends an agent hunting.

**A record is not a hold; a live process is.** A stale record (holder gone) is cleared and the
stand-up proceeds, exactly as the CLI does — otherwise one ungraceful teardown anywhere in a
package's history would make that topology permanently un-`up`-able over MCP. That clearing is the
only thing this verb writes to run-state, and it is a reap of someone else's litter rather than a
claim: a warm hold still mints no record of its own
(docs/design/mcp-mode.md#held-visible-via-status-not-ps).

Left open deliberately: whether a warm `up` should ATTACH to a live detached hold rather than refuse
it. The engine already rehydrates from the record's `value` on the CLI attach path
(docs/design/topologies.md#attach-binds-by-name), so a warm server could hold a topology it did not
provision — but that is a call about who the holder *is*, and getting it wrong means two processes
believing they own one teardown. Refusing is the honest answer until that is decided, and it is what
the CLI already does.

## 34. An unreadable record is treated as no record, which is the wrong way to fail

<!-- backlog: unparseable-runstate-record-reads-as-no-hold recorded=2026-08-20 -->
**A run-state record prova cannot PARSE reads as no hold at all, which is the fail-open direction.**
`runstate::read` is `serde_json::from_str(...).ok()` and `list` silently drops anything that will
not deserialize, so a record that exists but does not parse makes a LIVE holder invisible: `status`
omits it, and MCP/CLI `up` sail past their guards and provision a second instance — the same cost as
[[mcp-up-does-not-see-a-detached-hold]] and [[mcp-status-cannot-be-aimed-at-a-package]], reached by a
third trigger.

Two ways to get there. `runstate::write` is a plain `fs::write` (truncate, then write), so a holder
killed mid-write leaves a truncated file while its process may still be alive. And version skew:
`Record.value` carries `#[serde(default)]` specifically so pre-attach records parse, which is the
right instinct applied by hand — a future required field silently blinds every older binary on the
machine to newer records.

Found while writing the stale-record proof for the `up` guard, where a fixture wrote `endpoints` as
a bare empty Lua table (`{}` — an object, see [[a-list-verb-returns-a-list]]). The record was
unreadable, so `up` proceeded and the proof passed for the wrong reason. A hand-written fixture is
not a crash, but it reached the production path the same way a crash would, and nothing anywhere
said the file was unreadable.

Shapes worth weighing: write atomically (temp file + rename) so a record is never half-there; and
treat unparseable-but-present as LOUD rather than absent — a record whose pid cannot be read cannot
be liveness-checked, so the safe reading is "something may be held here", reported rather than
dropped. The second half is the one that closes the fail-open; the first stops manufacturing the
input.

## 35. A topology fixture is file-local, so a full run built the same world N times

<!-- claim: topology-fixture-is-file-local -->
**A registered topology can say `scope = "run"`, and then a run provisions it ONCE and every
declaring file binds that instance** (docs/design/topologies.md#run-wide-topology-is-provisioned-once).
`prova learn topologies` states the default plainly: a `prova.topology(...)` in a proof file "is a
fixture — local to the files that declare it". At suite scale that meant a package whose proofs
span several files, each declaring the same registered topology, provisioned that environment once
per file. Measured on ybor-studio via `docker events`: three proof files declaring the docker
topology and two declaring the kind topology turned an eleven-container world into **33 container
creations plus a cluster**, 364s, for one bare `prova`. The machine-scoped locks serialized the
duplicates, which made the waste safe and also made it slower.

Held topologies already deduped — every file attaches to the one live instance — so the field
workaround was to hold first (`prova start <name> && prova`). That inverts the promise of the cold
path: CI and a fresh checkout pay N× for the suite the author runs warm, and nothing in the output
says so (each provision narrates independently; nothing frames the repetition as repetition).

**Why the engine, and why the registration.** User-land cannot express this: each proof file
evaluates in its own Lua state, and `require` cannot smuggle one live instance across them. Nor can
the asking worker provision it — a state (and the teardown closures a factory parks on its scope)
dies with its suite, so whoever holds a run-wide instance must outlive every suite. Hence a holder
thread, and hence the definition must be the `[topologies]` REGISTRATION: a fresh state can only
reach a factory through `require`. A file's declaration of a run-wide name is the demand, not the
definition — exactly as under attach.

**Why opt-in rather than automatic for every registered name.** Implicit sharing would silently
change two things under working suites: the environment starts accumulating state across files, and
the value each file sees becomes the JSON projection (a `client` userdata does not cross a state).
Both are real trades, and only the package's own author can make them — so the intent lives in the
manifest, one line, per package.

Found live: `prova` on ybor-studio after the suite grew its second kind-topology file; the author's
own report was "it appears to create the same topology more than once — ideally this would be run
wide." The same report surfaced a neighbour, now warned about rather than discovered:
`--fresh` beside a live holder of a FIXED-name topology (`kind create --name ybor-studio`) collides
on creation, and the fresh run's teardown then reaps the holder's cluster
(docs/design/topologies.md#fresh-over-a-holder-is-announced).

## 36. The run-wide projection turns a container handle into its debug string

Under `scope = "run"` (and presumably in the attach record it reuses), a `ContainerResource`'s
`container` field crosses the projection as a STRING — which reads as data and invites use —
but the string is the handle's tostring (`Container: 0xbb30c84e8`), a Lua address with no
meaning to anything. A proof that fed it to `docker exec` got "No such container", one step
after the type check said string-and-therefore-fine.

`prova.Container` already carries `{ id: string }`, so the honest projections are: the id
itself (then `docker exec <id>` works from any file, which is exactly the escape hatch a
data-only projection wants to leave open), or omitting the field entirely (nil fails the
reach at the source instead of two layers later). The debug string is the one wrong choice —
it is the shape of usable data with none of its meaning.

Found adapting ybor-studio to scope = "run": the workaround is factory-exported id scalars
(`containers = { crdb = crdb.container.id }`), which works but re-states per package what the
projection could state once.
