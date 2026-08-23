# capabilities — what the host can do, and what a test needs it to

A **capability** is a fact about the *environment*, not the code: a daemon that answers, a tool on
`PATH`, a client compiled in, an OS. Tests say what they need; prova checks the host.

```
prova capabilities          # the host probes + everything THIS package declares or references
prova capabilities docker   # explain ONE: what it means here, what ran, what came back
```

## Declaring one: `[capabilities]` in prova.toml

A name plus a **factory** — the grammar `[topologies]` uses. Exactly one selector per entry.

```toml
[capabilities]
docker  = { intrinsic = "docker" }                       # prova's own checker, said out loud
gpu     = { package = "env", capability = "gpu" }        # a Lua predicate, from a package
java    = { command = "java", version = ["-version"], stream = "stderr" }
kubectl = { command = "kubectl", version = ["version", "--client"],
            pattern = "GitVersion:\"v([0-9.]+)\"" }
kind    = { command = "kind" }                           # PATH + the `--version` heuristic
"*"     = "error"                                        # probe (default) | warn | error
```

**`command`** — reach for this first; most capabilities are "is this tool here, which version".

| key | meaning |
|---|---|
| `command` | the executable (also the PATH-presence check when `probe` is absent) |
| `probe` / `expect` | args for the availability check (exit 0 ⇒ available) / require this output |
| `version` | args for the version query; `false` = no version concept |
| `stream` | where the tool talks: `stdout` (default), `stderr`, `both` |
| `pattern` | regex over that output; first capture group, else the whole match |

`{ command = "kind" }` behaves exactly as an undeclared `kind` already did — declaring a tool is never
a behavior change. `stream = "stderr"` is how `java -version` becomes readable at all; a `pattern`
narrows, and the parser still normalizes (`v1.30` → `1.30.0`). `retries` is for a daemon that hiccups.

**`package`** — for a fact no name-and-version can express (a GPU, a licence, a kind cluster).
Export it under `capabilities` in a package; return `true`, a version string, or `false`:

```lua
local M = { capabilities = {} }
function M.capabilities.gpu() return probe_cuda() end   -- true | "2.4.0" | false
return M
```

`capability = "gpu"` resolves to `capabilities.gpu` — the namespace *is* the advertisement. Use
`factory = "other.path"` to reach elsewhere, `options = {...}` to pass an argument. It lives in a
package, never a proof file: `must_run` is checked **before** any proof loads, and an exported
function is one a proof can **call directly** — which makes it testable.

**`intrinsic`** — says "not overridden" in the one file a reader consults, and aliases a built-in
(`dockerd = { intrinsic = "docker" }`). Built-ins work undeclared: `docker` (a daemon that answers *and*
runs Linux containers), `github` (`GITHUB_TOKEN`), `unix`/`windows`, `network`/`internet`, the compiled-in
`http`/`sqlite`/`grpc`/`graphql`/`yaml` — and **anything on `PATH`**, with no declaration at all.

## Two directions: requires (skip) and must_run (fail)

- `requires = { "docker" }` on a test/suite/topology — **skip** when unmet. The box without docker
  runs everything else and stays green. A version rides along: `"dotnet >= 9"`, `"node ^20"`.
- `[run] must_run` / `[profiles.ci] must_run` — **fail** (exit 2) when unmet, before anything runs.
  A profile's *guarantee* about its environment. Only bare `prova` is opportunistic.

Neither is ever silently green. A **misconfigured** capability — typo'd constraint, bad regex, or an
undeclared name under `"*" = "error"` — errors rather than skips: a gate that never matched reads green.

## Closing the vocabulary, and overriding a built-in

`"*"` chooses what an undeclared name means: `probe` (the open default), `warn` (probe, and print
pasteable TOML for each), `error` (refuse). `warn` is the migration rung. Strictness governs only
names **prova does not define** — a bare `unix` still needs no declaration.

An entry may redefine `docker` (or any built-in) here — refused once, because a companion predicate
changed a word's meaning silently and a manifest entry does not. Only the root manifest governs, and
the report marks the row `OVERRIDES the built-in`: never assume a name means what it does elsewhere.

**Not a capability: intent.** "Someone asked for this expensive class" — use `switch = "<class>"` (off
unless thrown with `-s`); `requires` stays for what the WORLD must provide. There is deliberately no
env-var selector: that is the pattern switches replaced. Nor is this the registry's discovery
vocabulary — a package advertises `keywords` for `prova packages <query>`.

**Migrating off `prova.lua`:** `runtime.capability(name, fn)` is deprecated and still works. Declare
`gpu = { package = "<pkg>", capability = "gpu" }`, move the predicate into that package under
`capabilities`, and drop the companion — it, `runtime`, `[run] config`, `--config`, and `PROVA_CONFIG`
retire together (a name declared in both resolves from the manifest).

See also: `prova learn topologies` (what a capability gates) · `prova learn running` (switches,
which these are not) · `prova learn drivers` (tools a driver needs)
