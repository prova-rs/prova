local ctx = Context.new()
ctx:prompt_text("Project name:", "project_name", { default = "widget" })
ctx:prompt_text("Description:", "description", { default = "a demo cli" })
-- The manifest is stored as `contents/{{ cargo_manifest }}` and renders to Cargo.toml: cargo scans
-- every Cargo.toml in a git dependency's checkout, and an un-rendered `{{ project_name }}` manifest
-- would print a package-name error into every build that depends on prova.
ctx:set("cargo_manifest", "Cargo.toml")
directory.render("contents", ctx)
