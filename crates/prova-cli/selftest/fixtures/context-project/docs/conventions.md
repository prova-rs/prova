# conventions — how proofs are written in THIS repo

- Name proofs after the behavior, not the endpoint: `orders_settle_test.lua`.
- Every service proof must cross-check the ledger table, not just the API reply.
- The staging topology is `orders`; hold it warm while iterating.
