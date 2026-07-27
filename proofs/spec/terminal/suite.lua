-- The terminal (pty) transport (docs/design/mocks-proxies-drivers.md). IMPLEMENTED and graduated
-- (flag-free): terminal_test.lua (driver + screen model + PATH-shadow mock) and proxy_test.lua
-- (terminal.proxy — the last matrix cell: interpose on an interactive CLI, record the session,
-- replay it with the asciinema-shaped cassette; the unix half of the cross-platform ConPTY story).
-- The ConPTY (Windows) twins land with a Windows runner + must_run.
suite.config{ name = "spec-terminal" }
