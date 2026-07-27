-- Resource tapping: prova.containerized composes with socket.proxy so ANY containerized
-- resource comes pre-interposed — transcripts and fault injection for Postgres/Redis/anything
-- TCP with one flag, zero protocol knowledge. The payoff the L4 wiretap was built for.
suite.config{ name = "spec-tap", requires = { "docker" } }
