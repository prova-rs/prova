# docs/design — current truth, held to the lifecycle

Two directories, two contracts:

- **`docs/design/`** is **current truth**. A design doc here describes the system as it is (or
  a direction explicitly labelled as such in its opening lines). It speaks today's vocabulary,
  and its normative statements are **anchored as claims** so the ledger can hold them.
- **`docs/plans/`** is **history**. A plan records what was decided and done at the time, in
  the vocabulary of its day. Plans are not swept when the language moves, and they carry no
  claim anchors — a claim in a historical document would be an obligation nobody can retire.

## Anchoring a design doc

The doc *claims* it; a proof *promises* it; the implementation *proves* it; the run *attests*
it. Concretely:

1. Find the normative statements — the sentences with a *must*, a *never*, an *always* that the
   system is supposed to honor. Not the argument, not the tour: the contract.
2. Anchor each: `<!-- claim: kebab-id -->` on the line above the paragraph. The paragraph below
   the anchor (to the next blank line) is the claim's text.
3. Bind what is already proven: `covers = "docs/design/<doc>.md#<id>"` on the existing proof
   that holds it. One proof may cover several claims; one claim may need several proofs.
4. What nothing proves stays **UNPROVEN in `prova owed`** — that is the point, not a failure.
   The ledger is the backlog; do not also keep a checklist.
5. Pin (`prova owed --pin`) only where the exact wording is the contract — "busy, never
   unsatisfiable" is a pin candidate; a paragraph of rationale is not.

Statements of taste, history, or aspiration get no anchor. A claim no proof could ever
discharge pollutes the ledger permanently — reword it into a contract or leave it prose.

`prova evidence` reports where this directory stands; bare `prova attest` is the gate.
