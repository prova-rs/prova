# Agent Ergonomics — frictions from the first external dogfood

Drafted 2026-07-16. Records what an **agent** actually hit driving Prova against a real target
(Minion — a local macOS daemon, deliberately *not* a container), and what to fix. This is the
"agentic PDD" requirement stated by the principal: *Prova must fit naturally into an agent's
toolkit, and must be learnable without reading its source.*

Ordered by cost. Every claim below is an observation from the session, not a hypothetical.

---

## 0. The meta-friction: Prova is not self-discoverable

**An agent learning Prova today must read Prova's source.** In this session, learning enough to
write one plugin required: `crates/prova-core/src/modules.rs` (to learn the `shell`/`fs`/`docker`
shapes), `plugins.rs` (resolution order), four `docs/design/*.md` (doctrine), and `library/*.lua`
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
a command **string**, so the same plugin, doing the same job against a local binary instead of a
containerized one, must hand-quote.

This bit immediately: passing arbitrary Lua source to `minion eval "<lua>"` is unquotable in
general (quotes, newlines, `$`). The workaround was to write the payload to a temp file and pass a
path — i.e. **route around the API**. Any plugin driving a local CLI with user content (SQL, JSON,
scripts) hits this.

**Fix:** accept an argv table in `shell.run`/`shell.spawn`, exactly as `container:run` does — same
rationale, same shape. Keep the string form (it is ergonomic for fixed commands).
`shell.run({ "minion", "eval", src }, { env = … })`. *This is an asymmetry between the local and
containerized halves of one SDK, and the containerized half is right.*

---

## 2. No project/manifest root primitive

A repo-local plugin must locate repo artifacts — `target/debug/miniond`, fixtures, testdata. There
is no primitive for "where is the manifest / project root". The options were: hardcode an absolute
path (unshippable), or depend on the process cwd (worked — the MCP runs at the repo root — but it
is an undocumented coincidence, and CI or a nested run breaks it).

`prova.workspace` exists and, by name, looked like the answer; it exposes only `create`.

**Fix:** expose the resolved package root (home) (e.g. `prova.root`, or `ctx.root`) — the directory
everything resolves against. One field. It is the anchor every path in a repo plugin needs, and the
runtime already knows it (it resolved the manifest to get here).

---

## 3. The Resource shape assumes Docker; a local daemon is a real shape

`prova.containerized` is the only constructor, and `ecosystem.md` is careful that the Resource shape
is "one shape, not the definition of a plugin". But **Minion is a Resource in every way that
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
local-daemon plugin exists.

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
> **How the error happened, because it is the thesis in miniature.** I read `plugin-system.md`
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
local plugins, the manifest, suites, and topologies in one place —

```
<repo>/prova/
  prova.toml
  plugins/<name>.lua
  tests/*_test.lua
  topologies/
```

Discovery today walks **up** from cwd looking for `prova.toml`, so `<repo>/prova/prova.toml` is
invisible from `<repo>` — the exact layout the convention wants. (`.cargo/`, `.github/`, `.claude/`
all solved this the same way.)

**Fix:** during the walk, at each level check `./prova.toml` **then `./prova/prova.toml`**. Keeps
the root-manifest layout working, makes the directory standard discoverable, and lets `[run] proofs`
and `[plugins]` resolve from the package root (home) (which they already do).

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
- **Remaining: 3** (`local_service`) — deferred until a second local-daemon plugin proves the
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

## 6. `learn` told a package with three plugins that it had none — **FIXED**

`prova learn project` and `learn doubles` both rendered:

> **Declared plugins**: none — add them under `[plugins]` in the manifest.

while `require("minion")`, `require("policy")` and `require("lib")` all worked — three local plugins
under the declared `[run] plugin_root`. The line reads `[plugins]` (external sources) only.

It is a true statement about one manifest key and a **false answer to the question being asked**.
`learn` exists so an agent need not read the source; for a package whose entire vocabulary is local
plugins, it actively denied that vocabulary existed. Worse in `doubles`, where the sentence lands
directly under "Plugins declared in this package add their facets to the vocabulary" — the local
`minion.daemon(ctx)` *is* such a facet.

**Fixed here.** Both kinds are listed, because `require("<name>")` does not distinguish them:

```
**Plugins** (`require("<name>")` in any proof):
  lib     local (.prova/plugins/lib)
  minion  local (.prova/plugins/minion)
  policy  local (.prova/plugins/policy)
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
daemon-as-server / plugin-as-client.

The concrete loss: a proof cannot stand in as the panel, so the full chain — Lua plugin chooses the
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
  plugins' worth of identical boilerplate — the doc's own bar for a constructor is met, if the second
  witness counts.

---

# Round three — 2026-07-24 (cross-repo integration: Minion consuming Aegis)

Driving a genuine two-repo integration: Minion's proofs reuse the sibling **Aegis** repo's own
`aegis` prova plugin (a hermetic Gate Authority + its CLI), declared cross-repo via
`[plugins] aegis = { path = "../aegis/.prova/plugins/aegis" }`. This is exactly the "plugins compose
across projects" story, exercised for real for the first time.

**The cross-repo MCP flow itself was frictionless** — worth stating, since the concern was that it
might not be. The Prova MCP was started in the Minion repo; `run` / `learn` / `introspect` all drove
the *Aegis* package cleanly via the `package` parameter (resolved fresh, ran the other repo's suite).
Nothing about targeting a second package by path got in the way.

## 9. A plugin could not locate ITS OWN repo — **FIXED (`plugin.dir`)**

The one real friction, and a sharp one. The `aegis` plugin needs to spawn `<aegis>/target/debug/aegis`.
It resolved that as `prova.root .. "/target/debug/aegis"` — correct when Aegis runs its own suite, but
**`prova.root` is the *consuming* package's root**, so the moment Minion consumed the plugin it
resolved `<minion>/target/debug/aegis`, which does not exist. A plugin reused cross-repo had *no
anchor on its own location* — only on the consumer's.

The workaround was ugly (pass `bin_dir` explicitly, computed from a `prova.root .. "/../aegis"`
sibling-layout guess — unshippable and repo-arrangement-dependent). The right fix is a primitive: a
plugin must be able to find itself.

**Fixed here.** Every plugin chunk now runs in a per-plugin environment carrying **`plugin.dir`** —
the directory its own file lives in (`plugins.rs`, `plugin_env`). So the `aegis` plugin resolves its
binary as `plugin.dir .. "/../../../target/debug/aegis"` and works **wherever it is consumed**, its
own suite or another repo's. The Minion integration proof then needs *zero* configuration:
`aegis.daemon(t)` just works. Proof: `proofs/plugins/plugin_dir_test.lua` (own dir is the plugin's
home, distinct from `prova.root`); verified end-to-end by Minion's `gate_attach_test` running with no
`bin_dir`.

Design note: it's a *per-plugin* binding, not a global, set the same way the private-dependency
`require` is (raw-set into the chunk env whose metatable falls through to the real globals) — so it
cannot leak to consumers, and a plugin without private deps now still gets its own env (previously
only plugins *with* private deps did). `plugin.dir` is the minimal primitive: the plugin's repo root,
fixtures, or binaries are all `plugin.dir .. "/…"` from there.
