-- The cassette engine (docs/design/mocks-proxies-drivers.md). Record/replay as a KERNEL facility
-- spanning every request→response transport, all IMPLEMENTED and graduated (flag-free guardrails):
-- cassettes_test.lua (http.proxy — the first), grpc_cassettes_test.lua (self-describing: the
-- cassette carries the reflected descriptors), socket_cassettes_test.lua (framed-turn VCR),
-- shell_cassettes_test.lua (record a real CLI once, replay credential-free). Full-duplex transports
-- keep the scripted-conversation caveat — no VCR cassette, by design.
suite.config{ name = "spec-cassettes" }
