# Lock starvation — a hold is a promise, and a promise needs a visible deadline

Status: **drafted 2026-09-02**, from the 2026-09-01 substrate incident recorded on both sides of
one wound: `agent-ergonomics.md#lock-waits-are-unbounded` (the waiter) and
`agent-ergonomics.md#a-hung-holder-never-releases` (the holder). Companion to
[architecture.md](../design/architecture.md) (`#locks-cross-instance`, `#lock-wrapper-verb` —
the mechanism), [verifiers.md](../design/verifiers.md) (`#conduct-heartbeat-not-deadline` — the
liveness primitive this reuses verbatim), and the `locks` learn topic (the teaching surface).

## The incident, from both ends

A `prova lock cargo --` conduct waited **50,925 s (~14 h)** for the token, then ran in 9.2 s. The
wait banked correctly (`waited 50925.4s for lock "cargo"`), which is exactly the diagnosis the
older item asked for — and it arrived a day late, naming nobody.

On the other end, a conducted proof that shelled `cargo test` 13× hung on one invocation and held
`cargo` for **1 d 22 h**, doing nothing. A manual `kill` of the holder released the lock
instantly, which is the confirmation that matters: the flock mechanism is sound, and nothing
anywhere bounds a live-but-hung holder.

The originating hang was a *caller* that spawned the conduct and failed to reap it on teardown
(substrate's session-abort, fixed there). Prova cannot stop callers leaking children. It can stop
a leaked child from holding a machine-wide house rule for two days.

## Three causes, and only the third was known

**1. Nothing bounds a conduct, and the scheduler holds the flock for the whole leaf.**
`shell.run` engages supervision only when a bound was asked for (`modules/shell.rs`, the
`run_supervised` branch); unbounded is the default. `ResourceTable::try_acquire` takes the flock
before the leaf and `release` drops it after (`engine/run.rs`). So one hung conduct pins a
cross-instance rule for as long as its process draws breath, and the pin is invisible.

**2. The flock is anonymous.** `locks.rs` opens `<token>.lock`, flocks it, and writes nothing.
Neither a waiter nor an operator can name the pid, the package, or the command. This is the odd
one out in this codebase: `barrier.rs` keys its arrival files by pid and sweeps them with
`kill(pid, 0)`; `runstate.rs` records a topology holder's pid and re-verifies liveness before
reporting. Locks have neither, so the only diagnostic available at 3 a.m. is `ps`.

**3. "The kernel releases a flock when its holder dies" covers death, not hang.** The reasoning
that downgraded the unbounded wait to a backlog item assumed liveness implies progress. A hung
holder is live, so the kernel never releases — and it is strictly the worse case, because nothing
else bounds it either. Blast radius is every instance on the box, across workspaces and CI, until
a human notices.

## The claim this lands

**A hold is a promise to everyone else on the box, and no promise may be broken silently and
indefinitely.** Three mechanisms, each already present in this tree in another form:

- the holder is **named** (barrier's pid-file convention, runstate's hint-vs-authority discipline),
- the hold is **supervised** (verifiers' bytes-or-CPU heartbeat, already proven not to false-kill),
- the wait is **narrated and bounded** by what it can actually see, never by a blind clock.

The heartbeat claim argues that a clock is illegitimate as a task budget. That still holds, and
the distinction that lets a wall bound in here at all is worth stating outright: **a clock is
illegitimate as a budget for the work and legitimate as a bound on how long you may exclude
everyone else.** Holding `cargo` is not doing work; it is preventing work.

## Part 1 — the holder polices its own hold

Per-lock liveness options on the declaration that already takes an options table
(`prova.writes(token, { scope = "machine" })` is the existing shape):

```lua
locks = { prova.writes("cargo", { idle_timeout = "15m", max_hold = "4h" }) }
```

- **`idle_timeout`** reuses `verifiers.md#conduct-heartbeat-not-deadline` verbatim — a window
  with no bytes on either stream **and** no CPU progress — applied to the *hold* rather than to
  one conduct. That predicate is already proven in this tree not to kill a silent compile, which
  is the whole reason to reuse it rather than invent a hold-specific heuristic.
- **`max_hold`** is the blunt outer bound for a body that spins rather than stalls (a `while true`
  in Lua accrues CPU, so the heartbeat will never call it idle).

`prova lock -- <cmd>` needs the same treatment for a second reason: it runs its child through a
bare `Command::status()` (`cmd_run.rs`), so it has no group isolation, no supervision, and no
lease. Routing it through the supervised path gives it `--idle-timeout` and the group kill from
`verifiers.md#conduct-process-group-reaping` in one move.

This is the part that fixes the incident at source: 15 minutes instead of 46 hours.

## Part 2 — the flock names its holder

A per-holder record beside the lock, keyed by pid: `<token>.holders/<pid>.json`, written after
acquiring, removed on release, swept by `kill(pid, 0)` exactly as `barrier.rs` sweeps. Fields:
pid, package, scope, mode, what (`run` / `provision` / `lock -- cargo test`), `acquired_at`, and a
`progress_at` heartbeat the holder refreshes.

**Two traps shape this, and both are load-bearing.**

**The lock file's inode must never change.** `flock(2)` binds the open file *description*, which
binds an inode. A temp-file-plus-rename over `<token>.lock` — the atomic-write discipline
`runstate.rs` just landed, and the reflex a reader of that commit would bring here — hands every
later opener a *different* inode to flock, so two processes both "hold" the token and the mutual
exclusion is silently gone. The record therefore lives in a sidecar directory, and
`<token>.lock` stays an empty file nobody ever writes.

**The record is a hint; the flock is authority.** This is the split
`agent-ergonomics.md#machine-wide-held-topology-index` already names for topologies, arriving
here for the same reason: two places recording one fact can disagree. Held with no record means
"held by a holder that did not register" — an external tool joining the convention from a
Makefile, which is a supported and documented thing to do — and that must be *said*, never read
as free. Unreadable follows `runstate`'s third state rather than collapsing into absence.

## Part 3 — the wait narrates while it waits, and can be bounded

`locks::hold` sits in a naked blocking `flock`, so by construction nothing can be printed between
"waiting" and "got it". Move the blocking flock to a helper thread; the calling thread narrates on
an escalating cadence and can abandon:

- **≥400 ms** — today's first line, threshold unchanged.
- **every 60 s** — `still waiting 6.0m for lock "cargo" — held by pid 12345 (prova run,
  /path/to/pkg) since 6.2m, last progress 5.8m ago`.
- **at the bound** — fail with those facts plus the literal remedy (`kill 12345`).

Abandoning is safe: the helper checks an abandoned flag when it finally wins and drops the handle
unheld. Keeping the *kernel's* queue (rather than polling `try_hold` in a loop) is deliberate in an
item about starvation — flock offers no fairness guarantee, but a barging poll loop would actively
remove what ordering the kernel does give.

**A waiter cannot break a foreign hold.** There is no API to release someone else's flock; only
killing the holder does that. So "fail loudly with the holder named" is the entire recourse
available to a waiter, which is what makes Part 2 load-bearing rather than cosmetic.

## Part 4 — `prova locks`

Every token at this package (and `--machine`), held or free, with the holder record and its age.
One command in place of `ps | grep` — the difference between a two-day outage and a two-minute
one. Teaching lands in the `locks` topic's "Waiting is visible" section, which currently promises
visibility the mechanism could not deliver.

## Two things that would have bitten a naive implementation

**Sniffing the holder pid's CPU is the wrong signal.** The holder is a prova whose *child* does
the work — prova itself burns ~0 CPU while cargo compiles — so a waiter reading the holder pid's
CPU would declare every healthy build hung. Progress must be heartbeated *by the holder*, which
already supervises its own conducts and is the only party that knows. Waiter-side CPU sniffing
survives only as a fallback for a holder that registered and then stopped heartbeating.

**`child_cpu_ticks` is private to `shell.rs`.** Its three native readers (procfs, libproc,
`GetProcessTimes`) are exactly what Part 1 needs; the change is a move to a shared module, not new
platform code. Note also that `flock` on Windows is a no-op stub (`locks.rs`), so all of this is
unix-real — Part 1's kill path should say so rather than imply coverage. The Windows twin stays
`agent-ergonomics.md#file-locking-is-a-no-op-on-windows`.

## Sequencing

**Parts 2 + 3 first, as one proof-carrying change.** Together they deliver everything the backlog
items ask for *diagnostically* without altering when any work dies. The waiter bound ships as a
mechanism with `--wait-timeout` and an env override, **defaulted off**, precisely so this change
cannot kill a legitimate CI queue.

**Part 1 second, on its own.** It changes what gets killed, so it carries its own proofs and its
own argument, and it is where the defaults below get turned on.

**Part 4 third.** Small, and worth landing once there is a record for it to print.

Note the interaction that makes this order safe: once Part 1 lands, a prova holder self-releases
at its idle bound, so the 14-hour *wait* ends without the waiter needing a default bound at all.
The waiter's bound only ever matters against a foreign holder that does not police itself.

## Open decisions — the three numbers

Recommended, not settled; Part 1 is where they bind.

| dial | recommendation | the honest cost of being wrong |
|---|---|---|
| hold `idle_timeout` | **15m**, on by default | on-by-default is the point, and it is the one that can kill someone's legitimate silent work |
| waiter bound | **30m**, `0` disables | a real CI queue behind a 45-minute build would fail rather than wait |
| `max_hold` | exists, **off** by default | one more dial; `idle_timeout` alone catches stalls but never a spin |
