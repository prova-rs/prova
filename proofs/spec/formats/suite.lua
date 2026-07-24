-- THE SPEC for the tech-first format modules (docs/plans/api-freeze.md §1): json / yaml / toml /
-- csv as sibling modules with encode AND decode, breaking prova.parse.json cleanly. Spec flags
-- are test-level (each open test carries its own); this file only names the suite.
suite.config{ name = "spec-formats" }
