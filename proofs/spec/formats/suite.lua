-- THE SPEC for the tech-first format modules (docs/plans/api-freeze.md §1): json / yaml / toml /
-- csv as sibling modules with encode AND decode, breaking prova.parse.json cleanly. Includes the
-- fidelity sentinels (json.null, json.array) decided day-one.
suite.config{ name = "spec-formats", spec = "api-freeze §1 — tech-first format modules" }
