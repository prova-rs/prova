# Autodidact — Prova teaches its own drivers

Drafted 2026-07-21. Companion to `docs/design/agent-ergonomics.md` (§0 is the requirements
document: *"Prova must be learnable without reading its source"*) and `docs/design/mcp-mode.md`
(the "cold agents" half). This plan turns the one flat embedded skill into a **progressive-
disclosure learning system** served identically over CLI and MCP, and repairs the introspection
surface so that everything the system teaches is *true*.

The bar: an agent with only the `prova` binary (as CLI, MCP, or both) — no source tree, no
prova-rs.github.io — can learn everything it needs: the PDD loop, the authoring surface, when to
reach for a Double vs a Proxy vs a Driver, how to create a plugin, which archetypes `init`
offers and when each applies, where this project keeps its proofs and plugins, and any extra
context the project's humans have provided.

---

## 1. What exists today (audit, 2026-07-21)

**Three sinks of one static document.** `const SKILL = include_str!("skill.md")`
(main.rs:2029, 203 lines): printed by `prova skill`, written by `prova skill --install` to
`.claude/skills/prova/SKILL.md`, served verbatim as MCP `instructions` (mcp.rs:563-570). No
templating; identical on every transport; nothing tests it against the real surface.

**Introspection is core-only, and not 100% true.** `prova.help([filter])` (engine.rs:2046) and
the MCP `introspect` tool (mcp.rs:608) both render `prova_core::help::core_entries()`, parsed
from the three embedded LuaCATS stubs (`library/prova.lua`, `modules.lua`, `double.lua` —
help.rs:19-26). Audit findings, ordered by severity:

1. **Documented-but-unregistered (introspection lies):** `prova.before_each` / `after_each` /
   `before_all` / `after_all` and the four `GroupBuilder` equivalents exist only in
   `library/prova.lua:287-296,546-555`. Nothing in engine.rs registers them — all eight raise
   "attempt to call a nil value". The stubs' enforcement tests check prose coverage, not
   existence.
2. **Registered-but-invisible: every plugin.** Resolved plugins' `library/*.lua` stubs are
   linked into `.luarc.json` for the IDE but never fed to `help()`/`introspect`. An agent asking
   `introspect` about `postgres.*` gets zero rows. `prova plugin lint` runtime-inspects a
   plugin's facets but its output is console-only, not wired into introspection.
3. **Registered-but-unstubbed:** `prova.workspace` (bundled module) — invisible to help, MCP,
   and the IDE (acknowledged at help.rs:23-25).
4. **Feature-gating untracked:** a build without `grpc` still introspects the full grpc surface
   — `introspect` never consults the compiled feature set or a Lua state.
5. **Parser blind spots:** `---@overload` dropped; field-closures fold into class signatures;
   same-name entries dedupe first-wins.
6. **Fix 0.3 from agent-ergonomics unshipped:** userdata still `tostring()`s as
   `ShellResult: 0xaa78...`; no `__pairs`.
7. **No proof covers any of this** — nothing in `proofs/` exercises `prova.help`, `introspect`,
   or the skill text.

**Discovery surfaces exist but are fragmented and CLI-only.** `prova init --list` (catalog:
built-ins `default`/`plugin` + user `~/.config/prova/config.toml [init.*]` overlay),
`prova up <url>` (remote advertised topologies), `prova --list` (test nodes), `prova plugin
lint`. None have MCP equivalents; none are *taught* — the skill doesn't tell an agent they
exist as discovery moves.

**Content corpus.** ~4,150 lines in `docs/design/`, ~1,410 in `docs/plans/`, ~6,400 across 48
pages on prova-rs.github.io. Audience split is clean: site = human tutorial/reference; design =
rationale for future sessions; **only skill.md is voiced at a consuming agent.** PDD is
triplicated (design thesis / site pitch / skill loop). "Doubles" as a heading exists nowhere —
this system debuts the Mocks/Proxies/Drivers taxonomy. Proxies and parts of the three-postures
model are design-only: **the catalog must never teach unshipped surface as shipped.**

**Manifest facts an agent needs are readable but not surfaced.** Where proofs go (`[run]
paths`, manifest-home-relative), where plugins are authored (`plugin_root`, no default),
declared topologies/suites/profiles — all in `prova.toml`, none served by any command. There is
no `context` key and no package registry yet (git shorthands + `[sources]` aliases only).

---

## 2. Design

Governing principle, applied throughout: **computed beats generated beats hand-written.**
Facts the runtime can resolve at the moment of asking are computed (slots); facts fixed at
compile time are generated from the code that implements them (verb table, schema docs,
stubs); hand-written prose is reserved for doctrine — and even that is fenced by build tests
and proofs (§2.8). Every hand-written restatement of a fact the binary knows is a future lie.

### 2.1 Shape: one knowledge system, three access surfaces

```
                    ┌── prova skill ────────── entry: the loop + a discovery map (CLI print,
                    │                          --install, MCP instructions)
  topic catalog ────┼── prova learn [topic] ── progressive disclosure (CLI)
  (embedded md +    │
  dynamic slots)    └── MCP tool: learn ────── same catalog, same rendering, {topic?} param
```

- **`prova skill` stays the entry point** and becomes a *router*: it keeps the crash-course
  core (the PDD loop, the test-file-in-one-screen, selection) and replaces its long reference
  tails with a **discovery map** — a table of `prova learn <topic>` / MCP `learn {topic}` /
  `introspect` moves and when to make each. The skill must remain self-sufficient for the 80%
  case (MCP instructions are the only text an agent is guaranteed to see), but depth moves
  behind one call.
- **`prova learn`** with no args lists the catalog: `topic  one-line hook`, exactly like
  `init --list`. `prova learn <topic>` prints that topic. Unknown topic → the list + exit 2.
  **Topics carry aliases** (`mocks`/`containers` → `doubles`, `topology` → `topologies`,
  `manifest` → `project`, …): the intuitive name resolves instead of erroring — an agent (or a
  human typing `! prova learn mocks`) should never bounce off our taxonomy. Aliases live on the
  `Topic` enum with a build test forbidding collisions (§2.8.1).
- **MCP `learn` tool**, `{ topic?: string }`, same code path, transport-spelled (§2.3.1).
  This is the eighth tool and the MCP twin of the whole docs site.
- **MCP resources, additionally** (decided 2026-07-21): the same catalog is published as
  protocol-native resources — `prova://learn/<topic>` with the hook line as description, plus
  `prova://skill` — off the same renderer. Dual exposure is the correct pattern, not either/or:
  the *tool* is primary because it is model-driven and works in every client; resources serve
  clients that surface them natively (@-mentions, resource pickers) at near-zero marginal cost
  (`enable_resources()` + list/read handlers over the `Topic` enum).
- One renderer, all sinks. Never hand the CLI and MCP different truths.

### 2.2 Topics: static doctrine + dynamic trailer

Each topic is an embedded markdown file (`crates/prova-cli/src/topics/<topic>.md`) with an
authoring contract (§2.4) and optional **dynamic slots** — `{{slot}}` placeholders substituted
at render time from the resolved environment. No template engine; the slot vocabulary is a
closed Rust enum (§2.8) — an unknown `{{slot}}` in a topic fails the build, not the render:

| Slot | Renders | Source |
|---|---|---|
| `{{init_catalog}}` | archetype key + description rows | `Catalog::load` (built-ins + user config) |
| `{{proof_paths}}` | where proofs go in *this* project | manifest `[run] paths` + suites |
| `{{plugin_root}}` | where local plugins are authored | manifest `plugin_root` (or "undeclared — set `[run] plugin_root`") |
| `{{plugins}}` | declared plugins: name, source, resource/library | manifest `[plugins]` + facet inspection |
| `{{topologies}}` | declared topologies + requires | manifest `[topologies]` |
| `{{profiles}}` | profile names + what they override | manifest `[profiles]` |
| `{{context_files}}` | project-provided context docs (§2.5) | manifest `context` |
| `{{mcp_or_cli}}` | transport-appropriate command spelling (§2.3.1) | render target |

No project found → each slot degrades to one imperative line ("no `prova.toml` here — run
`prova init` or pass `project`"). This is how a topic is **always true for this project**: the
doctrine is compiled in, the facts are computed at the moment of asking. Prose never restates
what the binary can compute — that is the anti-drift rule that made help.rs work.

### 2.3 The topic taxonomy

Thirteen topics. Classification: **[S]** static doctrine · **[D]** dynamic slots · **[S+D]**
both. Distillation sources in parens.

| Topic | Kind | Teaches | Sources to distill |
|---|---|---|---|
| `pdd` | S | The practice. What "take a PDD approach" means: proof first, red is correct, implement to green, never weaken a proof, commit proof+impl together. What makes an artifact a proof (executable, black-box, self-provisioning, machine-legible, durable). | design/proof-driven-development.md, skill loop |
| `project` | S+D | This project's shape: manifest location + schema tour, where proofs go, plugin root, suites, profiles, `prova.lua` companion, context files. *The first topic an agent should read in a repo.* | manifest.rs schema, site/prova-toml.md, plans/layout.md |
| `init` | S+D | Bootstrap. The archetype catalog ({{init_catalog}}), when to reach for which entry, `--answer/--switch/--defaults/--headless`, user catalog extension via `~/.config/prova/config.toml [init.*]`, never-clobber rule. | init.rs, catalog.rs, site/scaffolding.md |
| `authoring` | S | The DSL: test/test_each/describe/group/flow, opts, matchers, snapshots, parametrization. Includes: *there is no before_each — fixtures hold setup+teardown; that is the model.* | skill.md §2, site/writing-tests, library/prova.lua |
| `fixtures` | S | Scopes (Test/Flow/File/Suite), laziness, caching, LIFO teardown, Context (`use/manage/defer/tempdir`), suite = one Lua state. | site/fixtures.md, design/suites.md, design/api.md |
| `doubles` | S+D | Mocks + containers as one category: test doubles. When a double *earns its place* vs testing the real thing; container doubles for starting apps; `http.mock`; `X.mock` facets available here ({{plugins}}). Shipped surface only. | design/mocks-proxies-drivers.md (terminate posture), plans/mocks.md, site/mocking-and-proxies.md |
| `proxies` | S | Interpose posture: observe/perturb a live stream. **Taught as direction, explicitly marked unshipped** — one screen: the model, what exists today (nothing), what to do instead now. | design/mocks-proxies-drivers.md |
| `drivers` | S | Originate posture: how a proof speaks a protocol at the SUT — http/grpc/graphql/shell/terminal; choosing the driver that matches the contract under proof (gRPC contract → grpc driver, not curl). | design/mocks-proxies-drivers.md, module refs |
| `topologies` | S+D | One definition, two verbs (`up`/test); declared topologies here ({{topologies}}); warm MCP runs (`up` → `run{topology}` → `down`); ownership/teardown rules; `prova up <url>` remote discovery. | design/topologies.md, site/topologies.md, mcp-mode.md |
| `plugins` | S+D | Using: declaration forms, `[sources]` shorthands, version pins, `-P`. What's installed here ({{plugins}}) and how to introspect their APIs. | site/using-plugins.md, plugins.rs forms |
| `plugin-authoring` | S+D | Creating: the facet grammar (`client/container/wait_for/mock`), Resource vs Library, `prova init plugin`, where they live ({{plugin_root}}), private deps, `plugin lint`, shipping a `library/` stub *so introspection sees you*. | site/authoring-plugins.md, design/plugin-system.md, plugin-composition.md, namespacing.md |
| `running` | S | Selection is the scalpel: `-k/--tags/--node/--last-failed`, `--list`, eval semantics, watch, CI formats (json/tap/junit), exit codes, `must_run`. | skill.md §6, site/cli.md |
| `mcp` | S | Driving Prova as an MCP: tool↔CLI mapping, warm-topology workflow, `project` retargeting, when to hold a topology vs cold-run. | mcp-mode.md, mcp.rs tool docs |

Plus **project context topics** (§2.5), which appear in the same `learn` listing under a
`ctx:` prefix. Reserved for the registry future: a `packages` topic ships only when the
registry does — the taxonomy leaves the slot, the catalog never advertises vapor.

### 2.3.1 Environment-conditioned rendering

The agent should know how to drive prova **as configured in its environment** — MCP, CLI, or
both. The renderer knows its transport, and every sink applies it:

- **Served over MCP** (instructions, `learn` tool, resources): moves are spelled as tools
  (`run{topology}`, `eval`, `introspect`, `learn`), with an explicit "CLI-only verbs" note for
  what has no tool (`init`, `ide setup`, `plugin lint`, `skill --install`) — the agent learns
  it must shell out for those, and that both surfaces share one engine and one project state.
- **Printed by the CLI** (`prova skill`, `prova learn`): moves are spelled as commands, with
  the MCP tool named wherever a warm equivalent exists ("iterating? if the prova MCP is
  configured, `up` + `run{topology}` beats cold `prova` re-runs").
- **The installed file** (`skill --install`) is static and cannot know at read time whether an
  MCP server is live — so it teaches the *decision rule itself*: "if `mcp__prova__*` tools are
  available, prefer them for eval/run/warm iteration; use the CLI for scaffolding verbs and
  when no server is configured." Environmental conditions change per session; the rule is
  what's durable.

Neither surface is deprecated in favor of the other: MCP is the better *iteration* surface
(warm topologies, no process spawn, structured JSON); the CLI is the only *bootstrap* surface
and the one CI uses. The `mcp` topic teaches the mapping in both directions.

### 2.4 Authoring contract (the register)

Agent docs are not user docs. Every topic obeys:

1. **Imperative, dense, zero narrative.** "Put proofs where `[run] paths` points." Never "In
   this section we'll explore". The site's tutorial prose compresses ~5–10× on distillation.
2. **One screen** (~40–80 lines) per topic. Depth costs a second `learn` call, never upfront
   tokens. If a topic wants two screens, it's two topics.
3. **Code shapes over prose.** A 10-line Lua example with tight comments beats three
   paragraphs.
4. **Decision tables for "when".** Condition → action, one line each (which archetype, double
   vs real, which driver).
5. **Never restate what the binary knows.** Live facts come from slots or by pointing at
   `prova.help()` / `introspect` / `init --list`. Prose carries doctrine only.
6. **Shipped surface only**, except `proxies`-style direction topics, which say so in their
   first line.
7. **Voice of skill.md** — it is the register calibration for every topic.

### 2.5 `context` — project-provided knowledge in the same channel

New manifest key:

```toml
context = ["docs/agent-context.md", "~/team/prova-conventions.md"]
```

- Manifest-home-relative paths; `~/` expands. Missing file = hard error at load (never a
  silently absent doc — matches the catalog's malformed-config stance).
- Each file surfaces in `prova learn` as `ctx:<stem>` and renders verbatim (first line of the
  file is its listing hook; frontmatter `description:` wins if present).
- The `default` init archetype grows an optional context-file switch so `prova init` can
  scaffold one — user preferences in `~/.config/prova/config.toml [init.default.answers]`
  flow through the existing answer precedence.
- This is how a team extends the autodidact system without forking it: their doctrine rides
  the same discovery rail as ours.

### 2.6 Introspection repair (make what we teach true)

Ordered; items 1–3 are prerequisites for shipping any catalog that claims truthfulness.

1. **Delete the eight phantom hooks from `library/prova.lua`.** Fixtures are the model;
   `authoring` teaches that explicitly. (If xunit hooks are ever wanted, that's a feature
   decision taken separately — the stub is not where features are proposed.)
2. **Stub `prova.workspace`** (`library/workspace.lua`, added to `CORE_STUBS`).
3. **Parity proof** (the keystone): a proof that walks every `prova.help()` entry, resolves
   its dotted/colon path against the real Lua environment, and asserts the callable/field
   exists. Introspection can never again advertise a nil. Second proof direction where cheap:
   registered globals absent from help (catches the next `workspace`).
4. **Plugins into introspection.** `help()`/`introspect` gain entries from each resolved
   plugin's `library/*.lua`, namespaced by alias. Stub-less plugins fall back to facet
   inspection (the `plugin lint` machinery) → at least `name.client/container/...` rows.
   MCP `introspect` gains the `project` param and resolves the environment it currently
   refuses to build.
5. **Feature-gate structurally:** `cfg`-gate the stub *text* per feature so a build without
   `grpc` embeds no grpc stubs at all — the invalid state (introspects-present,
   actually-absent) becomes unrepresentable rather than filtered (§2.8).
6. **Fix 0.3:** `__tostring`/`__pairs` on crossing userdata (`ShellResult{ code=0,
   stdout=42B }`) — closes the probe class the ergonomics log identified.
7. **Skill/topic reference lint as proof:** every backticked `prova <verb>` in skill.md and
   topics parses against the real dispatch table; every `{{slot}}` is in the vocabulary;
   every topic listed renders non-empty. The docs test themselves the way the stubs do.

### 2.7 MCP parity decisions

- Add: `learn { topic? }` (this plan). `introspect { filter?, project? }` extended per §2.6.4.
- **Not** adding an MCP `init` tool yet: scaffolding writes a project, which an agent does
  equally well via CLI; the *knowledge* gap (what archetypes exist, when to use each) is
  closed by `learn init`'s `{{init_catalog}}` slot. Revisit when the registry lands.
- `list` should return `{ path, tags, requires, file }` (mcp-mode.md line 43 promised it;
  selection-by-tag over MCP is blind today). Small, in scope here because `learn running`
  teaches tag selection.

### 2.8 Enforcement ladder — make undocumented features unrepresentable

Docs that *can* drift *will* drift; the before_each bug shipped through a stub suite with
enforcement tests, because those tests could only assert prose coverage, not existence.
Prefer, in order: **(a)** the invalid state doesn't compile, **(b)** it fails the build's
tests, **(c)** a proof catches it. Everything in §2.6/§2.7 slots into this ladder; the
structural moves:

1. **Topic registry is an enum.** `enum Topic` with a per-variant `include_str!` and required
   hook line; the `learn` listing, the skill discovery map, and the renderer all derive from
   exhaustive matches. A topic without content, or content without a topic, cannot compile.
2. **Slot vocabulary is an enum.** Topics are parsed at build time; `{{slot}}` not in the enum
   fails a compile-adjacent test, and every enum variant must have a renderer arm
   (exhaustive match — a new dynamic fact can't exist without its no-project degradation).
3. **Verb table replaces the hand-written HELP const.** One table of
   `{ verb, summary, learn_topic }` drives dispatch, `--help`, and the skill's discovery map.
   A new subcommand physically cannot exist without a one-line summary and a topic home —
   today's hand-rolled dispatch + separate `const HELP` is exactly the two-places shape that
   forgot nothing so far only by luck.
4. **Manifest schema self-documents.** Doc comments on the serde `Manifest` structs become
   descriptions (schemars is already a dependency via MCP); a build test fails on any
   undocumented field. `learn project`'s schema section renders from it — adding a
   `prova.toml` key without docs becomes unrepresentable.
5. **Feature stubs are cfg-gated** (§2.6.5) — absence is structural.
6. **End-state for the Lua surface: invert stub-as-source.** agent-ergonomics chose the
   hand-written stub as the single source (a registry "would have been a second place to
   write every summary") — right call then, but before_each shows its ceiling: a stub cannot
   prove its subject exists. The end-state is **registration-carries-docs**: registering a
   Lua function requires `(signature, summary)` at the callsite, and that registration emits
   the binding, the help entry, *and the generated LuaCATS stub*. One writing site; phantom
   entries and unstubbed modules both become unrepresentable. This is a large refactor of
   engine.rs/modules.rs — staged behind the parity proof (§2.6.3), which pins the same truth
   at level (c) until the inversion lands, module by module.
7. **Plugins:** `plugin lint`'s missing-stub *warning* stays advisory for third parties but
   escalates to an error in official plugins' CI — the ecosystem's version of the same rule.

### 2.9 Outward flow — the same sources update the user docs

Everything above generates *inward* (into the binary). The identical sources render *outward*:
an `xtask docs-export` emits Docusaurus-ready markdown for the site's reference section —
`reference/cli.md` from the verb table, `reference/prova-toml.md` from the manifest schema,
`reference/lua-api/*` + module pages from the stubs (later, from registration docs), the
scaffolding page's catalog table from the built-in catalog. The site's reference section
becomes regenerated, not hand-maintained — closing the inventory's finding that the site
duplicates itself and drifts from shipped reality ("Status: Design phase" in a 0.3.1 README).
Narrative/tutorial pages stay hand-written; only reference is generated. Site CI can diff the
export against the checked-in pages to catch staleness the way `.version` stamps catch stub
staleness.

---

## 3. Milestones (PDD: each lands proofs-first)

**Status 2026-07-21 (same-day implementation run): M0–M5 shipped**, proofs-first throughout.
Deviations and remainders:

- **M1**: MCP resources shipped in M1 (not deferred to M6) — `prova://learn/<topic>`,
  `prova://skill`, plus the startup package's `ctx:*` docs; one `learn::answer()` path serves
  CLI, tool, and resources.
- **M2**: the skill kept its full crash course and gained the discovery map; the deeper
  tail-shedding into topics remains available but was not forced — the MCP instructions must
  stay self-sufficient. The reference lint covers skill + topics, inline and fenced.
- **M4 shipped the stub rail only**: a resolved plugin's `library/*.lua` merges into `help()`
  and MCP `introspect` (read at call time off `RunConfig::help_roots`). Deferred: the
  facet-inspection fallback for stub-less plugins, a `package` param on `introspect`, and
  §2.6.5 feature-gated stubs.
- **M5 deviation**: a declared-but-missing context doc is LOUD in `learn` (marked in the
  listing; exit 2 naming the path on read) but does not hard-error manifest load — breaking
  test runs over a docs file inverted the priority. `[run] context` (wrong placement, TOML
  makes it representable) is silently ignored; a lint would close it.
- **M6/M7 open**: Fix 0.3 tostring/`__pairs`, MCP `list` enrichment, §2.8.4 schema self-docs,
  §2.9 docs-export, §2.8.6 registration-carries-docs inversion.

- **M0 — Truth repair.** §2.6.1–3: phantom hooks deleted, workspace stubbed, parity proof
  green. *Proof: help↔runtime parity walk.*
- **M1 — Topic engine.** `Topic` + `Slot` enums (§2.8.1–2) with aliases, `prova learn` (list +
  print), `learn` MCP tool + MCP resources off the same renderer, slot renderer with
  no-project degradation, transport-conditioned spelling (§2.3.1). Seed with 4 topics: `pdd`,
  `project`, `init`, `doubles`. *Proofs: catalog lists = embedded set; every topic renders;
  `learn mocks` resolves to `doubles`; unknown topic exits 2; MCP learn/resource returns the
  MCP-spelled rendering of the same topic (drive `prova mcp` over stdio via shell.spawn).
  Build tests: slot closure, renderer exhaustiveness, alias collision.*
- **M2 — Skill as router + verb table.** skill.md keeps crash-course core, gains the
  discovery map, sheds reference tails into topics; §2.8.3 verb table drives dispatch/HELP/
  discovery map. *Proof: §2.6.7 reference lint (now against the verb table, not prose).*
- **M3 — Remaining topics.** Distill the other nine per §2.4. *Proof: reference lint covers
  all; one-screen budget asserted (line count).*
- **M4 — Plugin introspection.** §2.6.4–5. *Proofs: postgres stub entries visible via
  introspect in a fixture project; stub-less plugin shows facet rows; parity proof extended
  over plugin entries.*
- **M5 — `context` key.** Manifest field, `ctx:` topics, archetype switch, hard-error on
  missing. *Proofs: fixture project with context file lists and renders it; missing file
  fails load.*
- **M6 — Polish.** Fix 0.3 tostring/pairs; MCP `list` enrichment; §2.8.4 manifest schema
  self-documentation. *Proofs: tostring shape asserted; list returns tags. Build test:
  undocumented manifest field fails.*
- **M7 — Outward flow.** `xtask docs-export` (§2.9) for cli/prova-toml reference; site CI
  staleness diff. Registration-carries-docs inversion (§2.8.6) begins module-by-module,
  parity proof standing guard throughout.

Fold outcomes into `docs/design/` when landed (agent-ergonomics §0 gets its closing entry;
mcp-mode.md gains the `learn` tool). Registry topic ships with the registry, not before.

---

## 4. Open questions

Resolved 2026-07-21: verb is **`learn`** (also pleasant interactively: `! prova learn mocks`);
MCP **resources ship alongside the tool** (dual exposure, §2.1) rather than being deferred.

1. Should `learn` with no project still show project-shaped topics (`project`, `topologies`)
   in the list, or annotate them "(needs a project)"? Leaning: show + annotate — the agent
   should learn they exist before bootstrapping.
2. Does `skill --install` also install topic files as skill references, or stay one file and
   rely on the binary at runtime? Leaning: one file + binary — installed copies drift, and the
   installed skill teaches the decision rule (§2.3.1) rather than snapshotting facts.
