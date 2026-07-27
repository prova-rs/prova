# Windows coverage via Parallels — local red-to-green for the ConPTY twins

**Status: captured 2026-07-27, not designed in detail.** The blocker this dissolves: the ConPTY
twins of the `terminal`/`shell.proxy` specs (see mocks-proxies-drivers.md § Status) are gated
`requires = { "windows" }`, so on this machine they *skip* rather than run red — no local
burndown loop can ever drive them green, and staging them as specs would park permanently-open
backlog. A Windows guest changes that.

## The idea

This machine runs Parallels, and this package already declares the `vm` topology through the
`prova-rs/prova-parallels` plugin (its own proof skips today without `PROVA_PARALLELS_IMAGE` —
the hook exists). A Windows base VM makes Windows a *local* execution vantage:

1. **Provision**: the parallels plugin stands up a Windows guest (`PROVA_PARALLELS_IMAGE`
   naming a Windows template), with the repo (or an artifact of it) reachable from the guest.
2. **Execute**: run `prova` inside the guest against the same package — the windows-gated specs
   stop skipping and go red; the burndown loop works exactly as it does here.
3. **Guarantee**: the guest's run uses a profile with `must_run = ["windows"]`, so a
   windows-gated test that *skips* there is a hard failure — the mechanism the capability
   system already ships (no new machinery).

Record-replay compounds this: ConPTY cassettes recorded in the guest once can be committed and
replayed deterministically on every platform (the cross-platform story in
mocks-proxies-drivers.md), so the guest run is periodic verification, not a per-run tax.

## What has to be true first

- A Windows VM template with: a prova build (cross-compiled artifact or in-guest cargo), git,
  and a shared/synced view of the working tree. The build question (cross-compile from host vs
  build in guest) is the main open design point.
- The parallels plugin's guest-exec surface is rich enough to launch `prova`, stream output,
  and propagate the exit code (check against the current plugin; extend there if not).
- Then: author the ConPTY twin specs (near-copies of the unix terminal/shell.proxy suites,
  gated `requires = { "windows" }`) — they become stageable the day the guest loop runs.

## CI

The lane needs a separate **Windows CI job** eventually (GitHub-hosted windows runner with
`must_run = ["windows"]` — no Parallels involved). Local-via-Parallels and CI-via-runner are the
same suite at two vantages; neither replaces the other. Also relevant: the known Windows
git-cache fetch bug ("Access is denied" fetching git plugins) is masked in CI by the selftest
hermeticity barrier — a real Windows lane is exactly where it stops hiding.

## Related

- docs/design/mocks-proxies-drivers.md — capability gating + the ConPTY screen-model argument.
- docs/design/burndown-lane.md — the lane doctrine this would feed.
