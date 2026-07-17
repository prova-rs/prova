# Plan: parallels — VM-style black-box testing, and the Linux proving ground

Design refs: `docs/design/ecosystem.md` (plugin shapes; the docker-exec pattern this mirrors),
`docs/design/namespacing.md` (facets; a VM is a provisioned resource), `docs/design/test-topology.md`
(`runtime.capability` — the gate), `docs/plans/mocks.md` §C2 (the immediate consumer).

## Two things wearing one name — keep them apart

"A Parallels plugin" is two genuinely different capabilities, and conflating them makes a muddled
design. This plan is mostly about (A), because it is what unblocks C2 today; (B) is real but deferred.

**(A) Parallels-as-Linux-target** — drive a VM to run a prova suite *inside* it. Local Linux CI. This
is not a resource in the `{ client, url, container }` grammar — it is *where the test runs*, not a
thing under test. It exists because of a finding C2 forced (below).

**(B) Parallels-as-resource** — `parallels.vm(ctx)`: provision a VM you drive and assert on, the way
`docker.run` gives you a container. A genuine plugin (a namespace with a provision facet), for
black-box testing something Docker cannot contain — a systemd target, a kernel module, a full-OS
install, a Windows box. Lives in `prova-rs/prova-parallels`, Lua over `prlctl` via `shell.run` — the
same shape as the docker-exec plugins, a different CLI. **Deferred**: its only consumer today is
"prove C2", which (A) does better; build it when VM-style testing has a real user.

## The finding that reframes it: *where prova runs* is a new axis

The grammar already hides *which* substrate a resource is (native vs docker-exec — `ecosystem.md`'s
"the tier is an implementation detail the grammar hides"). C2 exposed a second axis it never had to
name: **where prova itself runs relative to the substrate.**

C2's proof is "a containerized SUT reaches a host-bound mock via `host.docker.internal`." For it to
mean anything, prova (running the in-process mock), the Docker daemon, and the container must share
one native-Linux host:

- **prova on the Mac, `DOCKER_HOST` → the VM's daemon** — a container resolves `host.docker.internal`
  to *the VM's* gateway, i.e. the VM. The mock is on the Mac. Unreachable regardless of how it binds.
  This proves a broken topology, not C2.
- **prova *inside* the VM** — daemon, mock, and container share one kernel. `host-gateway` resolves to
  the address the host is actually reachable at, and a `127.0.0.1`-bound mock is genuinely off that
  interface. The true native-Linux case, and the only one where the mutation check means anything.

So (A) is not a convenience — it is a *correctness requirement* for the proof. And it generalizes:
for a kind cluster or two services in a remote cluster, the same rule holds — prova runs next to the
daemon, not next to you. Get the seam right for Parallels and "run the suite in the cluster" is the
same move with a different launcher.

## (A) — the Linux harness (the immediate work)

A script/target, not a plugin: `scripts/vm-linux-proof.sh <testdata.lua>` — ensure the VM is up and
provisioned (Docker + a current Rust toolchain), sync the working tree in, build `prova`, run the
given suite inside, report. **Measured on the existing `Ubuntu 24.04 ARM64` VM, aarch64 matching the
Mac Studio host:**

- `prlctl start` + `prlctl exec` drive the guest as root; `prlctl exec` forwards stdin, so the source
  syncs as `tar c … | prlctl exec … tar x` — no shared-folder or SSH setup.
- Ubuntu's apt `cargo` (1.75) is **too old to read the v4 `Cargo.lock`** ("requires
  `-Znext-lockfile-bump`") — the harness installs a current stable via `rustup`. Docker via
  `apt install docker.io` gives a **native `linux/arm64` daemon** — the whole point, no Desktop VM in
  the middle.

### The capability gate — exactly the `runtime.capability` use the ask predicted

`runtime.capability("parallels", …)` in a `prova.lua`, verdict a version string so
`requires = { "parallels >= 20" }` works:

```lua
-- prova.lua beside the harness suite
runtime.capability("parallels", function()
  local r = shell.run({ "prlctl", "--version" })          -- "prlctl version 20.4.2 (55999)"
  if not r:ok() then return false end
  return r.stdout:match("version%s+([%d.]+)")             -- "20.4.2" → comparable
end)
```

`requires`, not `must_run`: on a machine without Parallels the VM proof **skips** (the honest signal —
"could not ask here"), and only a profile that *guarantees* Parallels (a specific dev box, a self-
hosted runner) turns absence into a failure. That is the whole `test-topology.md` contract applied:
the test states a need, the profile states the guarantee.

## (B) — the resource plugin (deferred, sketched)

```lua
local parallels = require("parallels")
local vm = parallels.vm(ctx, {
  image = "ubuntu-24.04",         -- a base template to clone (linked clone; disposable)
  cpus = 2, memory = "2G",
  wait = { ssh = true },          -- readiness: guest tools / ssh answering
})
vm:run({ "systemctl", "is-active", "nginx" })   -- exec in the guest, like container:run
vm.ip                                            -- reachable address on the host network
```

- **Facet:** `vm` is the provision verb (a VM is not a `container`, so `container` would misname it;
  `namespacing.md` allows a namespace its own extras as long as the trio is not renamed). It returns a
  resource: `{ url/ip, vm = <handle>, run, stop }`, `ctx:manage`d like any other.
- **Driving:** `prlctl clone --linked` from a template, `prlctl start`, `prlctl exec` for `vm:run`,
  `prlctl stop && prlctl delete` on teardown. All shell, no native code — a legal third-party plugin.
- **Readiness is a contract** (per `topologies.md`): "ready" must mean the guest can answer, not that
  `prlctl start` returned — the VM analog of the false-ready lesson. Gate on `prlctl exec true`
  succeeding, or ssh/guest-tools up.
- **OrbStack needs no plugin.** It *is* a Docker daemon prova already drives via `DOCKER_HOST`; its
  value here is a **third row in the substrate matrix** (Docker Desktop vs OrbStack vs native Linux
  disagree about `host-gateway`), not a new resource.
- **Trigger** (`north-star-roadmap.md` discipline): build (B) when a real suite needs to black-box a
  system Docker cannot hold. Until then it is speculative; the `prlctl`-driving code (A) writes is the
  reusable part, and (B) inherits it when a consumer appears.

## Build sequence

- **A.1 — the harness script. DONE.** `scripts/vm-linux-proof.sh` — ensure/provision (Docker +
  rustup)/sync/build/run in the VM. Idempotent; `prlctl`-gated (no-op without Parallels). Validated
  on the `Ubuntu 24.04 ARM64` VM.
- **A.2 — the C2 e2e proof. DONE.** `testdata/c2_e2e.lua` — containerized SUT reaches the host mock
  via the vantage; the loopback mutation passes on native Linux and self-skips on Docker Desktop
  (where it would fail). Wired into `cargo test` (`tests/c2_e2e.rs`, `failed==0 && passed>=1`, so the
  Mac skip and the Linux pass both satisfy it) and run fully by the harness in the VM.
- **A.3 — the gate.** `scripts/vm-linux-proof.sh` no-ops without `prlctl` today (the shell-layer
  form). A `runtime.capability("parallels", …)` companion is the prova-native form for a Mac-side
  suite that drives the VM as a test — deferred with (B), which is where a VM-as-test-subject lives.
- **B — deferred**, per the trigger above.

## Verify

- **A:** `c2_e2e.lua` green inside the VM; the loopback-mutation test **passes on Linux and would fail
  on Docker Desktop** — that platform divergence *is* the proof the vantage is load-bearing.
- **A (honest-skip):** on a machine without Parallels, the gated harness reports **skipped**, not
  passed — `test-topology.md`'s "a skip is not a pass".
