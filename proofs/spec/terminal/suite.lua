-- The terminal (pty) transport (docs/design/mocks-proxies-drivers.md): the Driver-primary
-- kernel transport for TUI/CLI proof — pty alloc + screen model + expect/wait_stable +
-- golden frames + the PATH-shadow mock. IMPLEMENTED and graduated; flag-free guardrails.
-- The ConPTY (Windows) twins land with a Windows runner + must_run.
suite.config{ name = "spec-terminal" }
