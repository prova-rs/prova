local ctx = Context.new()
ctx:prompt_text("Project name:", "name", { default = "demo" })
ctx:prompt_int("Port:", "port", { default = 8080 })
directory.render("contents", ctx)
