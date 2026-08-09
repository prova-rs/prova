-- The whole ut leg is one opt-in class: conducting the deputy compiles the workspace, so it must
-- never fire because a person typed `prova`. One declaration gates every proof in this directory
-- (docs/design/manifest.md#switches-not-env-capabilities); the `ut` profile throws it.
suite.config { switch = "ut" }
