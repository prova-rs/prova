# Capabilities — the declared vocabulary of host facts

Drafted 2026-08-23. A capability is a fact about the **environment**, not the code: a daemon that
answers, a tool on `PATH`, a client compiled in, an OS. Tests state what they need (`requires`);
a context states what it promises (`must_run`); prova checks both against the host.

This doc records how that vocabulary is **declared**. The short version: a capability is a named
factory registered in `prova.toml`, exactly as a topology is — and the separate companion file it
used to need is gone.

## What this replaced, and why the old shape was wrong

The vocabulary used to have three layers with three different declaration sites:

1. **Built-ins** — `docker`, `github`, `unix`, `windows`, `network`, `internet`, plus the
   compiled-in native clients (`http`, `sqlite`, `grpc`, `graphql`, `yaml`). Hardcoded in Rust.
2. **Anything on `PATH`** — an unknown name fell through to a binary probe, versioned by running
   `--version` and taking the first version-shaped token. No declaration at all.
3. **Registered predicates** — `runtime.capability(name, fn)` in a **separate Lua companion file**
   (`prova.lua` beside the manifest, or wherever `[run] config` / `--config` / `PROVA_CONFIG`
   pointed), loaded pre-suite.

Layer 3 was the problem, and not for a small reason. It was a special file, holding a special
global (`runtime`), reachable through a special resolution path, valid nowhere else in the system —
one concept with its own everything. Nothing else in prova works that way: topologies, suites,
dependencies, specs, and placement are all declared in the manifest or in the proof tree. A
capability predicate was also the only piece of project configuration that **could not be tested**,
because a function that exists only inside a file the runtime loads for itself is not addressable
by anything that could assert on it.

Layer 2 had a quieter problem. The `--version`-and-take-the-first-number heuristic is right often
enough to feel principled and wrong often enough to matter: `java -version` writes to **stderr**,
`kubectl version --client` needs a flag, `terraform version -json` needs parsing. When the heuristic
missed, the only escape was to write a Lua predicate — which meant reaching for the special file to
say something as ordinary as "the version is on the other stream."

## The model: a name, a factory, and options

<!-- claim: capabilities-declared-in-the-manifest -->
A capability is declared in the manifest's `[capabilities]` table, mapping a name to a factory and
its options. This is the same shape `[topologies]` uses, and deliberately so: one registration
grammar for "a thing my package provides under a name."

```toml
[capabilities]
docker  = { intrinsic = "docker" }                       # prova's own built-in checker, said out loud
gpu     = { package = "env", capability = "gpu" }        # a Lua predicate, exported from a package
java    = { command = "java", version = ["-version"], stream = "stderr" }
kubectl = { command = "kubectl", version = ["version", "--client"],
            pattern = "GitVersion:\"v([0-9.]+)\"" }
kind    = { command = "kind" }                           # PATH + the `--version` heuristic, explicit
```

<!-- claim: exactly-one-selector -->
Every entry names exactly one **selector** — `package`, `command`, or `intrinsic` — and an entry
with none, or with more than one, is a config error rather than a guess. The selector picks which
registry the factory comes from; `options` (Lua factories only) is handed to it as its second
argument, as with topologies.

### The `package` selector — a Lua predicate that can be tested

```toml
[capabilities]
gpu = { package = "env", capability = "gpu" }
license = { package = "env", factory = "capabilities.license", options = { tier = "pro" } }
```

<!-- claim: the-capabilities-namespace-is-the-advertisement -->
`capability = "gpu"` resolves to `capabilities.gpu` in the package's namespace — the convention *is*
the advertisement, so a package publishes a capability by exporting it under `capabilities` and
nothing else. `factory = "capabilities.license"` is a direct dotted path, for your own packages
where there is no contract to mediate. Exactly one of the two, same rule as topologies.

A `[[package.capabilities]]` advertisement table (the sibling of `[[package.topologies]]`) was the
first design and is deliberately *not* built. It buys the same encapsulation the convention already
gives — the consumer names `gpu`, never a path — while costing an ordering hazard: an advertisement
must be read from the providing package's manifest, which means capability resolution could no
longer happen where the rest of the manifest resolves, and `must_run` needs the vocabulary early.
A convention plus an escape hatch is the cheaper half of that trade.

<!-- claim: predicate-lives-in-a-package -->
A capability predicate lives in a **package** — a `require`-able module — never in a proof file.
This is not symmetry for its own sake: `must_run` is a precondition checked before any proof file
is loaded, so a capability declared inside a proof would not exist at the moment it is needed. The
manifest plus the resolved package set must be sufficient to answer the whole vocabulary.

That constraint is also what makes the predicate testable, which was the original complaint. An
exported function is an ordinary function:

```lua
-- .prova/packages/env/init.lua
local M = { capabilities = {} }
function M.capabilities.gpu() return probe_cuda_version() end   -- true | "2.4.0" | false
return M
```

```lua
-- proofs/env/gpu_predicate_test.lua
prova.test("the gpu predicate reports a version when the device is present", function(t)
  t:expect(require("env").capabilities.gpu()):equals("2.4.0")
end)
```

### Topologies have two doors; capabilities have one

<!-- claim: capabilities-have-one-door -->
A topology declared in a proof file is a fixture, local to the files that declare it, and
*deliberately* not addressable from outside the run — `prova up` resolves `[topologies]` and nowhere
else. Capabilities have no such second door: there is no proof-file form of a capability
declaration, and `requires` never sees a name a proof invented. The vocabulary is project-wide by
nature, so a file-local one would be a name that meant different things in different files — which
is the thing a capability exists to rule out.

### The `command` selector — the declarative probe

Most capabilities are "is this tool here, and which version." That deserves to be sayable in TOML.

| key | meaning |
|---|---|
| `command` | the executable (also the PATH-presence check when `probe` is absent) |
| `probe` | args for the availability check; exit 0 ⇒ available |
| `expect` | require `probe`'s trimmed output to equal this (case-insensitive) |
| `version` | args for the version query; `false` = this capability has no version |
| `stream` | where to read the version from: `stdout` (default), `stderr`, `both` |
| `pattern` | regex over that output; first capture group, else the whole match |
| `retries` | retry the probe this many times with backoff, for a daemon that hiccups |

Absent `probe`, availability is "an executable of that name is on `PATH`". Absent `version`, the
version is `--version` parsed by the first-version-token heuristic — so `{ command = "kind" }` is
exactly what an undeclared `kind` already did, written down. Absent `pattern`, the same heuristic
runs over whichever stream `stream` names, which is enough for `java -version` and most of the
rest; `pattern` is for when the number needs picking out of structure.

<!-- claim: version-false-cannot-satisfy-a-constraint -->
`version = false` declares that a capability has no version concept, and a constraint against it is
**unsatisfiable** — never quietly satisfied. The honest answer to "is this ≥ 9?" when there is no
number to compare is "cannot confirm", and a gate that cannot confirm must not wave the suite
through. The same rule already governs a probe that fails to produce a parseable version.

### The `command` selector can express the intrinsics — which is the test of the model

The built-in `docker` capability is not just "docker is on PATH": the daemon must answer *and* run
Linux containers, because Docker on Windows in Windows-container mode answers `docker info`
perfectly happily and then cannot pull `postgres:16-alpine`. A suite saying `requires = { "docker" }`
means "I am about to run a Linux image", so that is what the gate has to check. In the declarative
vocabulary:

```toml
docker = { command = "docker",
           probe   = ["info", "--format", "{{.OSType}}"], expect = "linux",
           version = ["version", "--format", "{{.Server.Version}}"],
           retries = 8 }
```

<!-- claim: intrinsics-are-expressible -->
That is a faithful spelling of the built-in, and a unit test asserts the two agree on this host.
The property matters beyond docker: if the intrinsics were *not* expressible in the vocabulary
offered to users, then `intrinsic` would be a privileged escape hatch rather than a named preset,
and every gap in the declarative form would be invisible from inside prova. The platform and
environment intrinsics (`unix`, `windows`, `github`, `network`, `internet`) are not command-shaped
by nature and are exempt.

### The `intrinsic` selector — saying "I mean prova's"

`docker = { intrinsic = "docker" }` declares that this name resolves to prova's built-in checker.
It is required for nothing — a built-in works undeclared, as it always has — and it exists for two
jobs: it tells a reader "not overridden" in the one file they would check, and it lets a built-in be
aliased under another name (`dockerd = { intrinsic = "docker" }`).

## Resolution: when each kind is probed

<!-- claim: lua-eager-command-lazy -->
Lua predicates resolve **eagerly**, once, at manifest load; `command` and `intrinsic` capabilities
resolve **lazily on first reference** and are memoized for the rest of the run.

The eagerness of the Lua side is not a choice. `Capabilities` stores answers, not closures: mlua
handles are `!Send`, each suite gets its own `Lua` state, `must_run` is checked before any suite
exists, and a capability that answered differently for two suites in one run would be a coin flip
rather than a capability. So the predicate runs at load and only its verdict survives.

The declarative kinds carry no such constraint — they are pure data, with no state to die — and
laziness there is worth having. Under a strict vocabulary a serious project declares every tool it
touches, and eagerly probing twenty of them would add twenty process spawns to every invocation,
including `prova --list`, to answer questions nothing asked. Memoization is per run and shared
across worker threads, so a `requires = { "docker" }` on fifty tests probes the daemon once.

This gives the declarative form a real advantage over the Lua form beyond terseness, which is the
right incentive: reach for `command` first, and for `package` when the fact genuinely needs code.

## The undeclared fall-through, and strictness as a ratchet

A name with no declaration and no built-in is probed as a binary on `PATH`. That is a feature —
`requires = { "kubectl" }` works with no ceremony, which is right for a small project — and it is
also how a name can mean "whatever was on the box", which is wrong for a serious one.

<!-- claim: wildcard-declares-the-fall-through -->
The `"*"` entry in `[capabilities]` declares what happens to an undeclared name: `"probe"` (the
default, and the behavior of every manifest already in the wild), `"warn"` (probe, and teach the
missing declaration on stderr), or `"error"` (refuse — an undeclared capability is a config error).

```toml
[capabilities]
"*" = "error"        # this package's vocabulary is closed
docker = { intrinsic = "docker" }
kubectl = { command = "kubectl" }
```

`"*"` cannot collide with a real capability name, since names are `[A-Za-z0-9_-]+` — which is why
the policy is a wildcard entry rather than a reserved key inside the same table or a second section
for one setting.

<!-- claim: strict-governs-only-undefined-names -->
`"*" = "error"` governs names **prova does not itself define**. A built-in may still be used
undeclared under strict mode.

That line was drawn deliberately, and the alternative — forcing `docker = { intrinsic = "docker" }`
and five more lines before a strict package can say `requires = { "unix" }` — was considered and
rejected, because it buys nothing. The value of strictness is "no name in my suite means something I
have not nailed down", and a bare `docker` *is* nailed down: prova defines it, and an override would
appear in the manifest, so its absence from `[capabilities]` is itself informative. Compiled-in
native clients are further out still — `http` is a fact about which prova you are holding, not about
your world, and the report already treats those as batteries rather than checks.

`"warn"` is the migration rung. It is how a package that wants a closed vocabulary gets there
without a flag day: run warm, collect the teaching lines, declare what they name, then close the
door.

## Overriding a built-in — now allowed, because it is no longer silent

Registering over a built-in used to be refused outright, on the grounds that `requires = { "docker" }`
must mean the same thing in every repo. The reasoning was sound against a *silent* override, which
is what the companion file offered: a predicate in a Lua file nobody reads, quietly changing what a
word means for every proof in the tree.

<!-- claim: overriding-a-builtin-is-declared -->
A `[capabilities]` entry may override a built-in, because the manifest is the one file a reader
consults to learn what a name means. Two containments keep it honest: only the **root** package's
manifest governs, so a dependency can never redefine `docker` for the package that consumes it; and
the `prova capabilities` report marks an overridden name as overridden rather than printing it as
an ordinary row.

<!-- claim: a-declared-no-is-final -->
A declared capability whose factory answers **no** is unavailable, full stop — it never falls
through to the layers below. Declaring what a name means and then having prova quietly ask a
different question is the worst of both mechanisms: under the old companion a predicate returning
`false` simply left the name unregistered, so a `mytool` predicate that answered no could still come
back available because a binary of that name happened to be on `PATH`.

<!-- claim: one-resolution-point -->
Every question about whether a capability holds — a test's `requires`, a suite's, a topology's, a
profile's `must_run`, and prova's own internal availability checks — resolves through the same
`Capabilities`, so an override is honored everywhere or nowhere. Prova's Rust unit tests are the
one deliberate exception: they self-gate on the intrinsic probe directly, because they run without
a manifest and so have no declaration to respect.

## Not a capability: intent

<!-- claim: intent-is-a-switch-not-a-capability -->
"Someone asked for this expensive class" is intent, not a host fact, so it is never a capability. A
test that must not fire unasked carries `switch = "<class>"`; `requires` stays for what the world
must provide. There is deliberately **no env-var selector** in `[capabilities]` — an env-var-probing
capability is exactly the pattern switches replaced
(manifest.md#switches-not-env-capabilities), and offering it as a declarative kind would rebuild
the thing that was torn down.

A `file` selector ("this licence exists at this path") is a legitimate host fact and an obvious
later addition; the factory registry makes adding one cheap. It is out of scope for the first cut
because nothing needs it yet.

## Deliberately not profile-scoped

`[capabilities]` is a property of the package, not of a run profile — like `[topologies]`,
`[dependencies]`, and `[globals]`. "In CI, `docker` means the remote buildkit" is a real want, and
the answer is still no: letting the *meaning* of a name vary by profile reintroduces exactly the
drift the old built-in refusal was protecting against, except now invisibly, since the report would
depend on which profile you asked under. `must_run` is already the profile-scoped layer, and it
scopes *policy* (what this environment promises) rather than *meaning*.

## What this deletes

The companion was one concept with its own everything, so retiring it retires all of it: the
`prova.lua` companion file, the `runtime` global and the metatable stub that made `runtime.*` a
clear error inside a test, `runtime.capability` itself, the `[run] config` manifest key, and the
`--config` flag and `PROVA_CONFIG` env override that selected it. `runtime` had exactly one member,
so nothing is left behind needing a new home.

The bridge comes down on a date, tracked in [deprecations.md](deprecations.md). Until then the
companion still loads and still registers, and each registration teaches its replacement on stderr:

```
prova: `runtime.capability("gpu", …)` in .prova/config.lua is deprecated — declare it in
       prova.toml instead:

           [capabilities]
           gpu = { package = "<your-package>", capability = "gpu" }

       Move the predicate into that package as an exported function, where a proof can call it
       directly. (`prova learn capabilities`)
```

<!-- claim: manifest-wins-over-the-companion -->
While both mechanisms exist, a name declared in `[capabilities]` wins over the same name registered
in the companion, and the shadowed registration is announced. Silent precedence between a
deprecated and a current mechanism is how a migration produces a mystery.

See also: `prova learn capabilities` (the agent-facing topic) · [manifest.md](manifest.md) (the
declaration site) · [topologies.md](topologies.md) (the registration grammar this mirrors) ·
[verifiers.md](verifiers.md) (`must_run` as a guarantee)
