-- Undefaulted fixture: a required prompt with NO default. A headless render (no answer, no default)
-- must fail cleanly rather than hang — the mechanism prova's `--headless` init relies on.
local context = Context.new()

context:prompt_text("Project Name:", "project_name", {
    help = "No default on purpose.",
})

directory.render("contents", context)
