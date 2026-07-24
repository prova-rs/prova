# Case study: supplanting Chainsaw (the Kubernetes stress test)

Drafted 2026-07-23. A deliberate stress test of Prova's *intrinsics*, prompted by the most common
first reaction to Prova from a Kubernetes team: "can this replace [Chainsaw](https://kyverno.github.io/chainsaw/)?"

The question is not "can we wrap `kubectl`" (trivially yes) but the sharp one: **can a team supplant
Chainsaw for *all* of their acceptance testing with a pure Tier-2 Lua plugin, no engine fork, no
native code?** If a Kubernetes plugin needs three new native primitives, that is a finding about
Prova, not about Kubernetes.

This doc records the audit and the verdict. It builds on [ecosystem.md](ecosystem.md) (the plugin is
a docker-exec / drive-the-CLI Tier-2 plugin) and [plugin-system.md](plugin-system.md).

## What Chainsaw actually is

Strip the YAML and Chainsaw is four things bolted to Kubernetes:

1. **Resource lifecycle** — apply / create / patch / delete, automatic namespace cleanup, `finally`.
2. **Steps** with `try` / `catch` / `finally` control flow.
3. **The differentiator: eventual-consistency *subset* assertions.** You write a *partial* resource
   (only the fields you care about); Chainsaw polls the live cluster until some object of that kind
   is a structural **superset** of your shape, or times out with a diff. This is the whole reason it
   is more than "kubectl in a for-loop."
4. **Kubernetes debuggability** — on failure it surfaces events, pod logs, and `describe` so a red
   test explains itself.

Its selling point ("low-code, declarative, no programming knowledge") is also its ceiling: the moment
a test must reconcile a CR **and** curl the endpoint the operator exposes **and** exec a migration in
a pod **and** check a Postgres row, Chainsaw is back to `script:` blocks — the bash testing it set out
to replace. That cross-boundary test is exactly Prova's north star, so the strategic prize is not
parity; it is the superset test Chainsaw structurally cannot write. But to *earn* the migration we
have to clear the parity bar first.

## The mapping: what the plugin looks like

The plugin decomposes the way the ecosystem doc predicts — `prova-kind` (a resource plugin) and
`prova-kubernetes` (drives the `kubectl` CLI, parses with `json.decode` / `yaml.parse_all`). Both
are **pure Lua**. `kubectl` is already a registered capability detector (binary on PATH), so
`requires = { "kubectl" }` gives graceful skip for free.

```lua
-- kind.lua — cluster lifecycle is a textbook Suite-scoped resource fixture.
local cluster = prova.fixture("cluster", Scope.Suite, function(ctx)
  local name = "prova-" .. ctx:uid()
  shell.run({ "kind", "create", "cluster", "--name", name }, { check = true })
  ctx:defer(function() shell.run({ "kind", "delete", "cluster", "--name", name }) end)
  local kubeconfig = shell.run({ "kind", "get", "kubeconfig", "--name", name }).stdout
  return k8s.attach(kubeconfig)                 -- a client-only namespace bound to this cluster
end)

-- a test: fresh namespace per test, shared cluster, torn down once.
prova.test("operator reconciles the CR", function(t)
  local k   = t:use(cluster)
  local ns  = k:namespace(t)                    -- Scope.Test fixture; defers `kubectl delete ns`
  k:apply(ns, MY_CR_YAML)

  -- the crown jewel: poll until the live Deployment is a SUPERSET of this shape.
  k:eventually(ns):get("deploy/my-app"):matches({
    status = { readyReplicas = 3, conditions = { { type = "Available", status = "True" } } },
  })
end)
```

Everything in that sketch except `:matches({...})` maps onto an intrinsic that **already exists**:

| Chainsaw feature | Prova intrinsic today | Status |
|---|---|---|
| kind cluster up/down | `prova.fixture(Scope.Suite)` + `ctx:defer`/`ctx:manage` (LIFO async teardown, runs once) | ✅ exists |
| apply/create/delete/patch | `shell.run` (capture code/stdout/stderr) or `container:run` | ✅ exists |
| command/script blocks | `shell` is a *first-class primitive*, not an escape hatch | ✅ stronger |
| try/catch/finally | Lua — real control flow | ✅ stronger |
| namespace cleanup / `finally` | scope teardown; every teardown runs even if one raises | ✅ stronger/general |
| multi-doc manifest input | `yaml.parse_all` (documented "as in k8s manifests") | ✅ exists |
| poll-until (eventual consistency) | `prova.retry(fn, {timeout, every})` | ✅ exists |
| pod logs `-f` / port-forward / watch | `shell.spawn` → background `Process` w/ `:output()`/`:stop()`/`:wait()` | ✅ exists (poll, no live stream) |
| shared cluster, per-test namespace isolation | `Scope.Suite` fixture + `Scope.Test` fixture depending on it (proven in `scopes_test.lua`) | ✅ exists |
| multi-cluster | multiple fixture values | ✅ more natural |
| `prova up k8s-dev` (reusable env) | `prova.topology` | ✅ exists, *beyond* Chainsaw |

So the structural bones — lifecycle, fixtures/scopes, retry, CLI-driving, background processes — are
**all present and idiomatic.** The plugin stays Tier-2 Lua. That is the headline: Prova's intrinsics
carry ~90% of Chainsaw with zero core changes.

## The gaps — and why they are the *right* gaps

Three things a kubectl-driving plugin cannot fake. The important property: **none of them are
Kubernetes-specific.** Each is a general-purpose intrinsic that many plugins want; Kubernetes is
merely the forcing function that surfaces them. That is the best possible outcome for a stress test —
it asks the engine to get *more general*, not to grow a K8s-shaped bump.

### Gap 1 — Subset / structural matcher + a table-aware diff  **(load-bearing, non-negotiable)**

This *is* Chainsaw's differentiator, and today Prova cannot express it.

- `t:expect(x):equals(y)` is **strict** deep equality — extra keys on either side fail
  (`tables_equal`, `engine.rs:1701`). It cannot say "expected ⊆ actual."
- `:contains(...)` is **flat** membership at one level, not recursive shape matching.
- On failure, the value printer renders **any table as the literal string `<table>`**
  (`display()`, `engine.rs:1848`) — no field dump, no diff. A failed structural assert would read
  `expected <table>, got <table>`, which is useless.

**Build:** a recursive subset matcher — call it `:matches(shape)` / `:contains_shape(shape)` —
"every key in `shape` is present in the subject and recursively subset-matches; extra subject keys
ignored; arrays matched element-wise (or as unordered contains, TBD)." Plus a **structured table
diff** for the failure message so a mismatch shows the offending path (`status.readyReplicas:
expected 3, got 1`). The building blocks are already there: `values_equal` gives the recursion, the
snapshot matcher already has an LCS line diff (`engine.rs:1309`) to model the renderer on. This is a
**localized addition to `engine.rs`**, not a ground-up effort.

This matcher is not remotely K8s-specific — it is what you want for asserting on any JSON API
response where you care about three fields out of forty. Chainsaw just makes its absence unmissable.

### Gap 2 — On-failure diagnostic hook  **(strong nice-to-have; the debuggability half of parity)**

Chainsaw auto-dumps events / `describe` / pod logs when a step fails, so a red test explains itself.
Prova today has **one flat `message: Option<&str>`** on `Event::NodeFinished` (`model.rs:109`) and
**no interception point** where a fixture registered against the scope can run "on failure, gather
`kubectl describe` + recent events + pod logs" and attach it to the report. `ctx:log` goes to stderr,
not the event stream (`engine.rs:1081`, explicitly "will become a Log event later").

**Build:** an `attachments` channel on the finished-node event + threading through the five reporters
(console/GHA/JUnit/TAP/JSONL), and an engine interception point in the failure path
(`engine.rs:3360`) where a scope-registered `on_failure(ctx)` handler contributes output. This is
more surface than Gap 1 (touches `model.rs` + reporters + `engine.rs`) but is again **fully general**
— every resource plugin wants "show me the container logs when the assert against this service
fails." The postgres/docker recipes already fake a weak version by packing diagnostics into the
raised error string (`shell.run{check=true}`, container-exit reporting); this promotes that pattern
to a real seam.

*Partial-credit path:* the plugin can ship *today* by having its own assertion helpers pack
`kubectl describe`/events into the raised error string, exactly as `shell.run` does. Ugly, but it
unblocks a v0 before Gap 2 lands.

### Gap 3 — Structured encode (`yaml.dump` / `json.encode`)  **(CLOSED — api-freeze §1)**

~~There is no encode primitive.~~ The tech-first format modules landed: `json.encode`/`json.decode`
(with the `json.null`/`json.array` fidelity sentinels), `yaml.dump`/`yaml.dump_all` round-tripping
`parse`/`parse_all` — so the table-first authoring style (`k:apply(ns, { apiVersion = ..., kind =
... })`) is available, and `yaml.dump_all` emits the multi-doc manifest stream directly.

### Deferred — streaming `:expect` over `shell.spawn`

Asserting on a specific line from `kubectl logs -f` today means polling `proc:output()` (a bounded
64 KB snapshot) with `prova.retry`. That *works*. A first-class line-oriented `:expect(pattern)`
surface is already **designed** as the `terminal` transport in
[mocks-proxies-drivers.md](mocks-proxies-drivers.md) (§ Driver, `:90-130`) but unbuilt. Not on the
critical path for Chainsaw parity — defer to that doc's roadmap.

## Verdict

**Yes — Prova has the right intrinsics.** A Chainsaw-supplanting plugin is a pure Tier-2 Lua plugin
(`prova-kind` + `prova-kubernetes`) driving `kubectl`, and the lifecycle / fixture / scope / retry /
background-process / topology machinery it needs **already exists and is idiomatic**. The stress test
surfaced exactly **one non-negotiable engine addition** (a subset matcher + table diff), **one strong
debuggability seam** (on-failure diagnostic attachments), and **one triviality** (structured encode).

The decisive result is that **all three gaps are general intrinsics, not Kubernetes special-casing.**
A subset matcher, failure attachments, and an encoder make *every* plugin and *every* API-shaped
assertion better; Kubernetes was just the forcing function sharp enough to expose them. That is the
signature of a healthy stress test: it pushes the core toward generality it wanted anyway, and the
domain stays entirely in a plugin.

## Build order

1. **`:matches(shape)` subset matcher + table diff** (`engine.rs`). Unblocks the differentiator;
   smallest, highest-leverage, useful far beyond K8s. Prove it with a proof suite over decoded
   `yaml.parse_all` fixtures — no cluster required.
2. **`yaml.dump` / `json.encode`** (`modules.rs`). Trivial; enables table-first manifest authoring.
3. **On-failure diagnostic attachments** (`model.rs` + reporters + `engine.rs`). The debuggability
   seam that closes the *pleasant-to-use* gap with Chainsaw. Ship the plugin's v0 on the
   error-string workaround before this lands.
4. **`prova-kind` + `prova-kubernetes` plugins** (external, `prova-rs/prova-kubernetes`), authored
   through `prova.containerized` where the resource shape fits, self-proven in their own `proofs/`.
5. *(deferred)* streaming `:expect` — via the `terminal` transport, on the mocks-proxies-drivers
   roadmap, not the parity path.

Steps 1–2 are days. Step 3 is the real engine investment. Step 4 is where the strategic payoff lands:
once the cluster is a fixture, the *same* proof that reconciles the CR can curl the operator's
endpoint and check the Postgres row behind it — the full-stack acceptance test Chainsaw cannot
express, which is the actual reason to switch.
