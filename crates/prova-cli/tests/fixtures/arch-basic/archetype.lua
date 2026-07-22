-- Basic fixture: one text prompt with a default, then a flat render into the destination.
-- The default (`demo`) lets a headless render succeed with no answer; a supplied answer overrides it.
local context = Context.new()

context:prompt_text("Project Name:", "project_name", {
    default = "demo",
    help = "Name stamped into the rendered files.",
})

context:prompt_text("Proof Dir:", "proof_dir", {
    default = "proofs",
    help = "Directory the proof suite lives in.",
})

directory.render("contents", context)

-- User-facing announcement — `prova init` must forward this to its stdout (in-test renders drop it).
output.print("scaffolded " .. tostring(context:get("project_name")))
