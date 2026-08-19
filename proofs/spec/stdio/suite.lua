-- The `stdio` kernel transport (docs/plans/stdio-transport.md): a CONVERSATION with a spawned
-- process over its pipes — the posture `shell.spawn` structurally cannot take, because its stdin
-- is nulled and a request/response SUT needs the next write to depend on the last read.
--
-- Sibling of `terminal`, differing only in pty allocation; sibling of `socket`, sharing the whole
-- turn model (framing / codec / where). Flag-free guardrails.
suite.config{ name = "spec-stdio" }
