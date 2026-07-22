-- State-echo fixture: proves init's generic package-state injection reaches an archetype. Each
-- prompt's default is the sentinel `absent`, so an injected answer (which pre-answers the prompt)
-- is distinguishable from no injection; the switch is read via `archetype.switches`.
local context = Context.new()

context:prompt_text("prova_package_root:", "prova_package_root", { default = "absent" })
context:prompt_text("prova_plugin_root:", "prova_plugin_root", { default = "absent" })

context:set("in_package", archetype.switches.is_enabled("prova:in-package") and "yes" or "no")

directory.render("contents", context)
