# Prova — The Package Manifest (`prova.toml`)

> The authoritative schema reference for the manifest and its resolution rules. The live,
> package-specific rendering of this material is `prova learn project` — that topic computes YOUR
> package's actual proof locations, packages, and topologies at call time; this doc is the durable
> contract behind it. Source of truth in code: `crates/prova-cli/src/manifest.rs` (schema) and
> `crates/prova-cli/src/home.rs` (discovery).

A prova **package** is a directory tree with a manifest. The manifest declares *what to run and
how* — so `prova` with no arguments runs the configured suite, and CI is just `prova`.

## Where the manifest lives (and what "home" means)

Four layouts; the **home** is the project ROOT in every case:

| Manifest file | Home (root) | Shape |
|---|---|---|
| `prova.toml` | the dir holding it | flat — prova at the root |
| `.prova.toml` | the dir holding it | flat, hidden — one file, out of sight |
| `prova/prova.toml` | the dir **above** `prova/` | nested visible — config tucked in a nook |
| `.prova/prova.toml` | the dir **above** `.prova/` | nested hidden |

<!-- claim: paths-resolve-against-home -->
Every manifest-relative path (`proofs`, `config`, `packages`) and generated artifact
(`.luarc.json`, the `.prova/var/` state directory) resolves against the home, never against the
manifest's own directory.

<!-- claim: nearest-manifest-wins -->
Discovery walks **up** from the working directory and the **nearest manifest wins**, so `prova`
runs correctly from anywhere inside the package — including from inside the nook (a bare
`prova.toml` found in a directory named `prova`/`.prova` roots at the parent).

<!-- claim: one-manifest-per-directory -->
Two manifest variants in one directory is an error ("keep exactly one").

<!-- claim: nested-package-isolation -->
A nested manifest deeper in the tree is its own independent package; proof discovery never
crosses into it.

## `[run]` and `[profiles.<name>]`

<!-- claim: profile-overlay-semantics -->
`[run]` is the default profile; `[profiles.<name>]` (selected with `--profile <name>`) overlays
it. Every field is optional. Overlay semantics: a profile field **replaces** the base's when
present — except `env` (base then profile, profile wins per key), `dependencies` (overlaid per
name, profile wins), and `must_run` (**union**, strictly additive: a profile promises *more* than
the package baseline, never less).

<!-- claim: knob-precedence -->
Where a CLI flag or environment variable exists for the same knob, precedence is uniformly
**CLI flag > env var > manifest > auto-detect**.

| Key | Default | Meaning |
|---|---|---|
| `proofs = ["proofs"]` | `["proofs"]` | Directory-**NAME** patterns (basename globs, NOT paths): every matching directory anywhere below the root holds `*_test.lua` / `*.test.lua` proofs. A matched directory owns its whole subtree (no re-matching inside). Discovery skips hidden dirs, `prova`, `target`, `node_modules`, `vendor`, `dist`, `build`, `testdata`, and nested packages. The pattern `"."` is the flat escape hatch: the root itself is a proof dir. |
| `packages` | *none* | THE directory of this package's own packages, root-relative. Deliberately singular, and no default — undeclared means nothing is scanned. Anything from elsewhere gets a name and a pinned source in `[dependencies]`. A profile's value replaces (never adds). |
| `config` | `prova.lua` | The Lua companion loaded once, pre-suite, with the manifest — where `runtime.capability(name, fn)` registers package-wide capability predicates. Path is home-relative. Override per run: `--config PATH` > `PROVA_CONFIG` env > this key. |
| `jobs` | `1` | Concurrent **suites** (`-j/--jobs` wins). Throughput only — it can never change what a run means. |
| `format` | `console` | `"console"` \| `"json"` (JSONL event stream) \| `"tap"`. `--format`/`--json` win. Never auto-switched when piped. |
| `color` | `auto` | `"auto"` \| `"always"` \| `"never"` — console color. `--color` > `PROVA_COLOR` > this key; `auto` additionally honors `NO_COLOR`/`CLICOLOR_FORCE` and never styles a non-terminal. |
| `progress` | `auto` | `"auto"` \| `"always"` \| `"never"` — activity narration during blocking pauses (image pulls, readiness polls, captured commands). **stderr only**, so it can never affect `--format json`/`tap`; that is why it is safe on by default. `--progress` > `PROVA_PROGRESS` > this key, and `--quiet` implies `never` unless `--progress` is explicit. |
| `quiet` | `false` | Only failures (with their header chain), the recap, and the tally. `-q/--quiet` can only *enable*. |
| `github` | `auto` | The GitHub Actions sink: `::error file=,line=` PR annotations + a `$GITHUB_STEP_SUMMARY` table, composing with any `format`. `"auto"` turns on when `GITHUB_ACTIONS=true` **and the run is the job's own** — annotations and the step summary are job-scoped resources, so a run nested inside another (`PROVA_RUN_DEPTH > 0`, stamped on every process prova spawns) leaves them to its parent and reports through its exit code instead. `"on"` forces the sink at any depth. `--gha` > `PROVA_GHA` > this key. |
| `junit` | *none* | Also write a JUnit XML report to this home-relative path (suite named after the package; file/line/timestamp/assertions attributes). `--junit PATH` wins. |
| `[run.env]` | `{}` | Environment applied before the run — the same suite targets ephemeral containers locally and real endpoints in CI by profile-swapping this table. |
| `[profiles.X.packages]` | `{}` | Profile-scoped packages overlaid on package `[dependencies]` (profile wins on a name). The principled home for CI-only capabilities: pinned in-repo, so `--profile ci` resolves identically everywhere. |
| `must_run = ["docker", "dotnet >= 9"]` | `[]` | Capabilities this context **GUARANTEES**, checked as a precondition before anything runs. Same expression grammar as a test's `requires` — but where an unmet `requires` skips, an unmet guarantee **fails the run** (exit 2): a broken environment must not read as a green suite. Unions across base + profile. |

## The other tables

| Table | Meaning |
|---|---|
| `[suites.<name>]` | An explicit suite: `paths = [...]` (dirs/files whose test files all share ONE Lua state — live `Scope.Suite` fixtures) plus optional `setup = "path/suite.lua"`. The zero-config alternative is a directory's own `suite.lua`, which groups that directory's test files (directory-scoped, not the subtree). |
| `[dependencies]` | `name → source`, package-wide. Source forms: a local path string; `"owner/repo@ref"`; or `{ git\|path, tag\|branch\|rev, module }` (`module` defaults to `<name>.lua` then `init.lua`). `require(name)` in any proof resolves through this. |
| `[sources]` | `alias → base` (`github:acme` or a base URL) so packages can say `"acme:prova-redis@v1"`. |
| `[topologies.<name>]` | `package` (required) + exactly one of `topology` (the package's advertised name) or `factory` (dotted path), optional `requires` and `options` (passed to the factory). Sugar for `prova.topology(name, require(package).<factory>)` — the name `prova up <name>` and proofs both address. |
| `[luals]` | `manage = "auto"` (default) \| `"always"` \| `"never"` — the `.luarc.json` pointer policy. `auto` creates when absent and reconciles non-destructively when present (silent in the steady state; a JSONC file it can't parse gets a hint). `never` is the right setting when a repo deliberately commits a hand-maintained `.luarc.json`. Annotations themselves always sync to the shared machine cache. |
| `[updates]` | Git-source freshness for `[dependencies]`: `interval` (default `"1d"`; `"12h"`/`"30m"`/bare seconds), `force` (also `-U/--update`), `retention` (default 90 days for unused materialized trees). `--offline` forbids the network entirely. |
| `context = ["docs/agent.md"]` | **Top-level**, not under `[run]`: team docs (home-relative, `~/` expands) served by `prova learn` as `ctx:<stem>` topics — the project's own doctrine on the same discovery rail. A declared-but-missing file is reported loudly. |

<!-- claim: switches-not-env-capabilities -->
**Opt-in test classes want a first-class switch, not an env-var capability.** Three gates in this
repo (`soak`, `quality`, `ut`) express the same intent fact — "someone asked for this expensive
class" — by conscripting the capability axis: a `runtime.capability` probing an env var, the env
var set in a profile's `[env]`, `requires = { "<class>" }` on every test, `must_run` in the
profile. Four pieces, three files, and `prova capabilities` misreports intent as a host gap ("ut
UNMET" on a machine that lacks nothing). Capabilities are for **world** facts; intent belongs on
the **selection** axis — the lane model's own distinction. The primitive: `switch = "<class>"` on
a test or suite (archetect consonance is deliberate — its `-s` switches toggle optional
functionality the same way; the two tools should rhyme). A switched test is **off unless thrown**
— fail-closed at the declaration site, unlike a tag whose exclusion must be remembered in `[run]
tags = ["!…"]`. Doors to throw it, matching the `heed_reminders` three-door shape: a profile
(`[profiles.ut] switches = ["ut"]`), the CLI (`prova -s ut`, ad hoc), `[run]` for a repo where a
class is on by default. The bare run reports switched-off classes as ONE summary line ("switched
off: ut (3), quality (3), soak (2) — throw with -s or a profile"), which teaches better than N
SKIP rows and truer than silent deselection. `requires`/`must_run` keep their proper job — world
facts only (`cargo-nextest`, `docker`) — and the strict-profiles increment composes unchanged.
Migration: the three gates convert, their `config.lua` registrations and profile env vars delete,
and `prova capabilities` stops listing pseudo-capabilities. The switch is one half of a dual pair,
and its complement already ships: tags + the `!` grammar (every selector axis, plus `[run] tags =
["!…"]` as the baked form) give **on-unless-excluded** — right for narrowing what legitimately
belongs to the default run (a flaky class quarantined for a week, a CI shard). Switches give
**off-unless-thrown** — right for classes that must never fire unasked. The design should present
them as the two postures of one membership model, and choosing between them is part of authoring
a class: "would a forgotten manifest edit running this by accident be a bug?" — if yes, it is a
switch, not a tag. **Membership doctrine (the drift concern):** a thrown switch AUTHORIZES a
class, it never widens scope — profiles keep selecting positively (`proofs`/`tags`), so a proof
added later cannot creep into `prova run ut`, and the bare run stays the total catch-all of
everything unswitch'd, so an unclassified addition always runs somewhere. The residual hole —
a switched class no profile throws, or a profile throwing a switch nobody declares — is an
alignment invariant in the `lane_surface_parity` mold: a unit test iterating declared switches ↔
profile `switches` lists, adopted into the account by the ut leg. **One primitive, standard
doors — not two mechanisms:** the switch is the authorization bit; `prova -s ut` and a profile's
`switches = ["ut"]` are its CLI and baked doors, the same shape as `--heed` vs `heed_reminders`.
The invocations differ by the ratified strictness split, not by mechanism: `prova run ut` is the
CONTRACT (exactly the profile's positive scope, its guarantees), `prova -s ut` is the COURTESY
WIDENED (the bare catch-all plus one class authorized). The CLI door is severable — it earns its
place only for multi-class sweeps (`-s ut -s soak`; profiles run one at a time), classes too
small for a profile, and ad-hoc iteration on one heavy test; if those never materialize, drop it.
**Open sub-decision:** does positively naming a switched test throw its switch implicitly? Exact
`--node` should (deselecting a test the caller named precisely is the swallowed-selector
dishonesty the reminders lane just killed); fuzzy `-k`/`--tags` must not (a grazing keyword must
never conduct a workspace compile by accident). Recorded 2026-08-09.

<!-- claim: runner-is-the-subject-not-the-conductor -->
**`[runner]` names the binary UNDER TEST; nothing re-execs — the binary you invoke conducts.**
The section's first life was a re-exec trampoline ("whichever prova was invoked, the one that
judges is the one the manifest names"), and it earned its retirement empirically: every
invocation paid a provision the verb rarely needed — a ledger read (`prova owed`) sat behind a
cargo build, and an MCP handshake with a 30s client budget died behind one while the
freshly-hopped server came up speaking into an abandoned pipe. The reframe: the trampoline's
whole purpose was TESTING, so testing is the only thing that pays for it. A **run** provisions
the declared subject just in time (`build`, skipped while `sources` are older than the LATER of
the provision stamp and the bin's own mtime — a direct `cargo build` of the subject IS a
provision; `-U` forces; failure is a loud exit 2 and nothing judges) and injects it as
`prova.bin`, so nested proofs judge this tree's build. **Everything else** — query verbs,
`prova mcp`, the tool in your hand — answers as the binary you invoked, immediately; a human
refreshes their own tools deliberately (`cargo xtask install`). Identity holds where it matters:
`binary_identity_test` guarantees nested runs and the suite AGREE on the subject, and the
subject is the tree's build whenever a run conducted it. A `prova.bin` child never re-provisions
(the depth guard inherits; empty-counts-as-unset re-arms a sandbox), and a declared-but-
unreadable `bin` is loud — a typo must never put the conductor in the subject's seat. The
provision is itself a build-tool invocation, so it holds the package locks the manifest
declares (`locks = ["cargo"]`, blocking-exclusive for the build's duration) — it can never
race a conduct holding `writes("cargo")` in another instance, and external tools join the
same rule by flocking the same file (the lock file is the contract — `prova learn locks`). The
consumer form (`runner = ">= 0.18"`, a version gate with auto-fetch) remains open — it wants
the registry's release channel.

<!-- backlog: subject-provisions-at-first-read recorded=2026-08-11 -->
**The subject should provision at first `prova.bin` READ, not at run start.** The subject model
made runs the only payers; the minimal form makes the *reaching proof* the only payer: a
selected proof that never touches `prova.bin` bears zero build cost, so "run this one proof"
costs exactly that proof — the granular-efficiency half of the platform sketch, applied to the
subject itself (`prova -k one_pure_proof` after an engine edit should not compile the engine).
Wants design, not just plumbing: the read is a Lua field access, so laziness is an `__index`
seam feeding a CLI-provided provisioner; concurrent first-reads under `-j` must single-flight;
and the provision itself must take the package's `cargo` lock — a lazy build racing a proof
that holds `writes("cargo")` would violate the exact house rule the locks exist to enforce.
Until it lands, a run's up-front provision is the honest over-approximation.

<!-- claim: switches-are-discoverable -->
**A switched class can never become a hidden test population.** Three sightlines, one per way of
looking: every bare run's tally carries the switched-off line (the classes are SEEN on the
default CI leg, every time); the tests lane lists switched tests as declared members (a run holds
them back, the medium still owns them); and `prova switches` answers the ledger question the
other two cannot — every declared class, how many leaves it gates, and **who throws it** —
`[run]`, the profiles that list it, or nobody, in which case the row says ad-hoc only (`-s`).
Orphanhood is surfaced, never forbidden: an ad-hoc-only class is a legitimate posture (`soak`),
but it must be a visible fact an agent and a reviewer can read, not an accident discovered when
someone finally asks why a gate never ran.

<!-- claim: spec-sources-are-queryable -->
**A package's spec sources must be answerable to whoever has to write one, and say which are
writable.** `[[specs.source]]` decides where prose obligations live, and `scan_roots()` is read all
over the CLI and MCP — but nothing *reports* it: `prova learn project` tells an agent where a new
proof, package, topology and local package go, and never where a new claim or backlog item goes;
the manifest table above does not list `[specs]` at all. So the one question that must be answered
before capturing an obligation — "where may I write it?" — is answerable only by opening
`prova.toml`, which is exactly the guess-the-file failure
`docs/design/mcp-mode.md#backlog-capture-is-a-taught-procedure` describes. Writability is the second
half and is not cosmetic: today every source is a local `directory`, read/write by construction (it
is what lets `prova backlog promote` rewrite an anchor in place), but `docs/plans/spec-sources.md`
holds room for `jira`/`github` sources, and an agent that writes an anchor into a read-only source
has captured nothing. Surface both — the sources, and which accept a write — on the discovery rail
that already exists (`learn project`'s "Where new things go"), not as a new verb. Recorded
2026-08-08.

## Worked example

```toml
[run]
proofs   = ["proofs"]           # every proofs/ dir below the root
config   = ".prova/config.lua"  # companion tucked into the nook
jobs     = 4
junit    = "target/prova.xml"

[run.env]
LOG = "info"

[profiles.ci]
jobs     = 8
must_run = ["docker", "dotnet >= 9"]   # CI guarantees these: unmet → FAIL, never skip
[profiles.ci.env]
CI = "true"

[dependencies]
postgres = "prova-rs/prova-postgres@main"

[suites.grpc]
paths = ["services/grpc"]
setup = "services/grpc/suite.lua"

[topologies]
stack = { package = "compose", topology = "stack", options = { file = "docker-compose.yml" } }
```

<!-- claim: empty-resolution-is-an-error -->
An empty resolution (no `proofs` matches and no `[suites]`) is an error, not a silent green run
— as is a `--profile` name that doesn't exist, or a selection that matches nothing (see
`--allow-empty`).

## The package vocabulary, and the spellings it retired

Everything is a package: the project under proof, the libraries it depends on, the topologies
they advertise — one `prova.toml` shape, worn different ways. The manifest says it in four
places: `[package]` is the package declaring itself, `[dependencies]` the packages it consumes,
`[run] packages` the directory of its own local packages, and a `[topologies]` entry names the
`package` providing its factory.

<!-- claim: deprecated-spellings-teach -->
The `plugin` spellings those replaced — `[plugin]`, `[plugins]`, `plugin_root`, `plugin =` in a
`[topologies]` entry, and the `prova plugin`/`prova plugins` verbs — are deprecated, not dead:
each still works for one release and warns once per process naming its successor (the
`spec` → `promises` pattern), and all of them retire together at 1.0. The canonical spellings
warn nothing.

## Field reports (Substrate gate-integration run, 2026-08-11)

<!-- claim: lane-time-budgets recorded=2026-08-11 -->
**A lane can state its time budget, and the run goes red past it — even all-green.**
`budget = "30s"` on `[run]` or a profile prices that lane's composition: when the suite's
wall time exceeds it, the run fails naming the overage and teaching the cure (heavy legs sit
behind switches), so a conduct-carrying proof landing in the wrong lane is caught at
placement time, not discovered at usage time. A budget never inherits — `[run]`'s binds only
the bare run, a profile's only that profile — because the heavy lanes are exactly the ones a
bare-run budget must not leak onto. Suite wall time only: the subject provision is not the
lane's composition. `prova run --list` shows each lane's budget as a chip. (Field report
2026-08-11: five graduated proofs landed switchless in Substrate's default lane and the
seconds-fast inner loop silently became a 10-minute gate, caught only by a human noticing.
Composes with the graduation moment: the promises→proves flip is when a proof acquires its
conduct cost and must name its lane.)

<!-- backlog: provision-refresh-respelling recorded=2026-08-12 -->
The runner provision force-rebuild answers to -U/--update -- the same spelling the registry refresh uses and the archetype-cache refresh will use, three refreshes wearing one flag. When -U unifies on "update cached assets" (registry.md#update-flag-means-cached-assets), the provision rebuild takes its own verb (e.g. --reprovision): the provision stamp already distinguishes a stale subject from a fresh one, so the flag exists only for the distrust case, and that case deserves a name that says which thing is being distrusted.
