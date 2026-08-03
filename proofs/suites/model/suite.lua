-- The suite-model proofs live in their own nested suite — which is itself the boundary rule at
-- work: a nested `suite.lua` ends the parent's reach, so nothing here joins the `orders` suite
-- above, and nothing here can see its `store`.
suite.config{ name = "suite-model" }
