-- The cassette engine (docs/design/mocks-proxies-drivers.md). cassettes_test.lua holds the
-- graduated http.proxy guardrails (modes, loud replay miss, record-time redaction — flag-free).
-- The open surface staged 2026-07-27: grpc_cassettes_test.lua (self-describing replay — the
-- cassette carries the descriptors), socket_cassettes_test.lua (framed-turn VCR on the L4
-- wiretap), shell_cassettes_test.lua (record a real CLI once, replay without binary/network/
-- credentials).
suite.config{ name = "spec-cassettes" }
