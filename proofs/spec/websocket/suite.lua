-- The websocket transport: the http upgrade path, full-duplex, message-framed by the protocol
-- itself. IMPLEMENTED and graduated (flag-free): websocket_test.lua (mock/driver, on_connect push,
-- §6 journal) and proxy_test.lua (websocket.proxy — the interpose posture, the last cell in the
-- transport matrix: direction-tagged transcripts + the fault vocabulary).
suite.config{ name = "spec-websocket" }
