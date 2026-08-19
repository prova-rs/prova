-- Durations refuse rather than drop (agent-ergonomics.md#unparseable-durations-are-dropped-not-
-- refused). The VALUE half of the closed-opts doctrine: the key set was already closed, so a
-- misspelled KEY is refused by name — this is the same guarantee one level down.
suite.config{ name = "spec-durations" }
