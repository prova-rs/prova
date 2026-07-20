-- Switch fixture: always render `contents/`; render `ci/` only when `--switch ci` is passed.
-- Proves switch plumbing end to end — the gated file's presence is the observable.
local context = Context.new()

context:prompt_text("Project Name:", "project_name", {
    default = "demo",
})

directory.render("contents", context)

if archetype.switches.is_enabled("ci") then
    directory.render("ci", context)
end
