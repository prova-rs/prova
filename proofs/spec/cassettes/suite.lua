-- The cassette engine (docs/design/mocks-proxies-drivers.md): record/replay as a KERNEL
-- facility — modes, matching, redaction live once; each transport contributes only its turn
-- model and match key. http.proxy is the first specialization. Tier A, spec-first.
suite.config{ name = "spec-cassettes" }
