local ctx = Context.new()
ctx:prompt_text("Project name:", "project_name", { default = "widget" })
ctx:prompt_text("Description:", "description", { default = "a demo cli" })
directory.render("contents", ctx)
