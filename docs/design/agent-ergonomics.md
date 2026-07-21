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
