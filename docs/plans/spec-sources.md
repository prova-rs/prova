# Spec sources — making `[specs]` a list of typed sources

Status: **design direction**, not built. Today `[specs] docs = [...]` scans project directories.
The itch: "docs" hardcodes one kind of source. Longer term, a spec's prose can come from a JIRA
project, a GitHub issue tracker, etc. — the project directory is the 97% case, not the only one.

## The shape

`[specs]` becomes a list of **typed sources**, each an entry type with its own attributes:

```toml
# terse shorthand for the common case (desugars to directory sources) — keeps the 97% one-liner
[specs]
docs = ["docs/design", "README.md"]

# explicit / typed form — one or more sources, repeatable per type
[[specs.source]]
type = "directory"
path = "docs/design"

[[specs.source]]
type = "jira"
project = "PROVA"
jql = "labels = spec"
```

Keeping `docs = [...]` as sugar for directory sources answers the itch without a verbose array-of-
tables for every trivial project: the word "docs" stops being *the* config and becomes one spelling
of one source type. Multiple directories, multiple JIRA sources, mixed — all allowed.

## The sticky part: writability

A source has a **capability**: read-only or read/write. This is what governs the whole feature, and
it is why the shape should be designed against the *first real remote source*, not now from the one
example we have:

- **`directory`** — read/write. `prova backlog promote` rewrites the anchor in place; this is what
  makes the in-place two-state toggle work. The 97% case, and the one that needs no auth story.
- **`jira` / `github`** — read/write is *conditional* (API auth + write scope) and often absent.
  The core unresolved question: **what does "promote a backlog item" mean when its source is
  read-only?** Candidates: (a) promotion is only offered for read/write sources; (b) a remote
  source is a read-only *reflection* whose state is managed in the remote tool's own workflow (a
  JIRA transition), and prova mirrors rather than mutates it; (c) promotion of a remote item mints a
  local claim shadowing the remote address — rejected as leaky. Leaning (a)+(b): directory is
  read/write; remote sources reflect, and their promotion happens in the remote tool.

## No implicit default at all — one explicit way

Decision (building now): **completely opt-in, no shorthand, no default.** prova takes a strong
stance on explicitness — you should be able to see what prova is doing and how it is configured. The
burden of knowing what to declare is real, but it avoids surprises and stops anyone paying for a
feature they never asked for. Conventions belong in **packages and init archetypes** (`prova init
project`, `archetect render <project>`) — captured repeatably — not in an implicit default.

- **No `[specs]` = no scan.** Unchanged; the load-bearing principle holds.
- **No implicit `docs/` default**, not even within opt-in. There is nothing to override or augment —
  the earlier default/override discussion is moot.
- **`[[specs.source]]` is the one way.** A typed source; today only `type = "directory"`.
- **`docs = [...]` is DEPRECATED** — the flat shorthand is a second spelling of a directory source,
  exactly the "two ways to say one thing" that "one way, one terminology" cuts. Kept working with a
  **deprecation warning** so existing projects migrate rather than break.
- **A spec verb run with no source configured warns and points to setup** (`prova learn spec`, and
  eventually a prova-rs.github.io entry once its identifiers are stable) — invoking the verb IS the
  signal you want the feature, so a pointer is help, not a lecture. Bare `prova` stays silent.

## Why defer the *remote* source types

The typed-source abstraction is best validated against a second, genuinely *different* source
(JIRA), because that is where the constraints that should shape it — auth, writability, id
addressing, rate limits — actually bite. Restructuring `[specs]` into a sources list now, with only
`directory` implemented, risks designing the abstraction wrong from a single example (premature
generalization). So: adopt the direction, keep `docs = [...]` shipping, and build the sources model
alongside the first real remote integration.

## Connection that already exists

`covers` already accepts external addresses like `jira:PROVA-142`, which `reconcile` treats as
**opaque** (unresolvable, deliberately skipped). A JIRA spec source is exactly what would make those
addresses *first-class* — resolvable obligations with text and state — instead of opaque strings.
That is the natural seam where sources pay for themselves, and a good trigger for building them.
