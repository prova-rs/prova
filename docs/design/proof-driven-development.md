# Proof-Driven Development — the practice Prova is an instrument for

> Companion to [`foundations.md`](foundations.md). That doc is the thesis behind the
> *runner*: the orthogonal primitives that decide whether Prova subsumes the
> acceptance-testing landscape. This doc is the thesis behind the *practice* — what changes
> about how software gets built when the author in the loop is increasingly an agent, and why
> Prova is the right shape for it.

## One line

**In Proof-Driven Development, "done" is not a claim — it's a proof that runs.**

## The name is the thesis

`prova` is not a coined product name that happens to evoke *proof*. It **means** proof —
trial, test — in Italian, Portuguese, and Spanish. `provare` is the verb: *to prove, to try*.
So the instrument and the practice collapse into a single root:

- **prova** *(noun)* — the artifact: an executable proof of a system's behavior.
- **provare** *(verb)* — what the author does in the loop: bring a system into existence and
  prove it works.
- **Proof-Driven Development** *(practice)* — you drive the work by producing proofs, not by
  asserting completion.

This is rare and it is load-bearing. The name doesn't need a decoder ring; the etymology *is*
the argument.

## What we are — and are not — claiming

We did **not** invent black-box testing. Hurl, Bats, Venom, Robot Framework, and goss already
own that category, and [`foundations.md`](foundations.md) says so plainly. Single static
binary, cross-platform install, a clean plugin surface — that's engineering, not a paradigm.
Planting a flag on "another language-agnostic test runner" earns a correct shrug.

What we **are** naming is a shift in *practice* that is already happening, unnamed, in the way
systems get built when an agent holds the pen:

> The deliverable is no longer a claim of completion followed by trust. The deliverable is a
> **proof that executes and goes green.** Verification stops being a step you append and
> becomes the unit of work itself.

## Why now: the author changed

Every prior acceptance tool assumes a **human** writes a spec to drive **human** development.
Two things shift at once when the author is an agent:

1. **Who holds the pen** — the agent authors the executable spec.
2. **What closes the loop** — the runner's feedback is machine-legible enough that the agent
   *iterates autonomously*, and the proof is the durable artifact, not scaffolding it
   discards.

There is a third shift that is specific to agents and is exactly Prova's layer:

3. **The system-under-test is a black box the author conjures into existence.** Unit
   frameworks (pytest, JUnit, `cargo test`) assume the code already exists *inside a
   language*. Agents increasingly generate *whole systems* — scaffold a repo, render an
   archetype, build it, boot it — and must verify at the **outside boundary**. That boundary
   is precisely where Prova lives: render → build → spawn → poke with shell + HTTP + filesystem
   assertions, with fixtures holding setup and teardown together.

## Old frame vs. Proof-Driven Development

| Old frame                                   | Proof-Driven Development                              |
|---------------------------------------------|------------------------------------------------------|
| A test is a check you *add* to code         | A proof is the *deliverable itself*                  |
| The agent says "done" (a claim)             | The agent hands you a proof (evidence)               |
| Trust, then verify                          | Verification *is* the unit of work                   |
| Humans write tests for human developers     | Whoever builds it — human or agent — ships the proof |
| Tests are scaffolding, often thrown away    | The proof is the durable artifact of the work        |

## The failure mode it forbids

The chronic, expensive failure mode of an autonomous agent is a **confident wrong "done"** —
a completion claim unbacked by evidence. Policing that behavior after the fact is a losing
game.

Proof-Driven Development reframes it from a *behavior problem you police* into a *category
error the workflow forbids*: **there is no "done" without a green `prova`.** Completion isn't
a sentence the agent emits; it's a proof it produces, that anyone — human, CI, another agent —
can re-run to reproduce the verdict.

## What makes an artifact a *proof* (not just a test)

Not every test qualifies. A proof, in this sense, is:

1. **Executable** — it runs and returns a verdict; it is not prose about behavior.
2. **Black-box** — it exercises the system from the outside, at the boundary a user or caller
   would, so a green result means the *system* works, not that an internal mock agreed with
   another internal mock.
3. **Self-provisioning** — it brings the system-under-state into existence and tears it down
   (fixtures), so the proof is reproducible from nothing, not dependent on a hidden warm
   environment.
4. **Machine-legible** — its output (JUnit XML, TAP, JSON, value-bearing assertion diffs) is
   structured enough that the loop can close without a human reading terminal scrollback.
5. **Durable** — it lives with the system as the standing evidence of what was proven, and
   re-runs to catch regressions.

These aren't new virtues. What's new is treating them as the **definition of "done"** rather
than nice-to-haves you get to after shipping.

## Why Prova is the instrument, not just *a* tool

Proof-Driven Development is a practice; it could in principle ride on any runner. Prova is
built for it on purpose:

- **A real language (Lua), not YAML/Gherkin.** Agents reason in code. The moment a proof needs
  a loop, a computed value, a conditional, or reusable scoped setup, declarative formats hit a
  wall — and agents hit it constantly. (See the programmability wall in
  [`foundations.md`](foundations.md).)
- **A real fixture model.** Provisioning the SUT — render, build, boot, teardown — is
  first-class, which is what makes a proof self-provisioning and reproducible.
- **Single static binary, cross-platform.** The agent (or CI, or a teammate) can *run the
  proof* anywhere with no runtime to install. A proof nobody can easily re-run is not much of
  a proof.
- **Machine formats + rich diffs.** The feedback is legible to the thing iterating on it,
  whether that's a person or a loop.
- **A plugin surface.** The long tail of "how do I bring *this* kind of system into existence"
  (archetype rendering, protocols, harnesses) is filled without forking — the same reason
  pytest won.

The instrument serves the practice: Prova exists so that producing a proof is the path of
least resistance, which is the only way "done means green" survives contact with real work.

## Boundaries and honesty

- This is a **practice with a name**, not a claim to have invented testing, TDD, or ATDD.
  (Note: *ATDD* already means **Acceptance** Test-Driven Development — we deliberately do not
  reuse that acronym.)
- PDD is **not** unit testing's replacement. pytest/JUnit own in-language unit testing; PDD is
  about the black-box acceptance boundary, which is exactly where agent-built systems need
  proving.
- "Proof" here is empirical, not formal-methods proof. A green `prova` is reproducible
  behavioral evidence, not a mathematical guarantee of correctness for all inputs. We use the
  word for what it earns: *this system was brought into existence and observed to behave as
  specified.*

## The sentence to remember

> **Agents don't tell you it works. They hand you a proof that does.**
