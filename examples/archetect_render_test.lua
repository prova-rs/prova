--- POC example: the `archetect` plugin — render an archetype in-process and assert on the output.
--- Run from the repo root: `prova examples/archetect_render_test.lua`
---
--- `archetect.render{...}` renders headlessly (no prompts): pass `defaults = true` to take every
--- default, and `answers` to override. It returns a tree handle rooted at the destination.

local greeting = "crates/prova-archetect/tests/fixtures/greeting"  -- local archetype (relative to CWD)

-- A file-scoped fixture: render once, share the tree handle across the tests below.
local rendered = prova.fixture("rendered", "file", function(ctx)
  return archetect.render{
    source = greeting,
    destination = ctx:tempdir(),
    answers = { name = "widget", port = 9090 },
    defaults = true,
  }
end)

prova.test("renders the parameter-named file", function(t)
  local out = t:use(rendered)
  t:expect(out:file("widget.txt")):exists()
end)

prova.test("templates the answers into the contents", function(t)
  local body = t:use(rendered):file("widget.txt"):read()
  t:expect(body):contains("Hello, widget!")
  t:expect(body):contains("Port: 9090")
end)

prova.test("reports the write operations", function(t)
  t:expect(#t:use(rendered).writes):never():equals(0)
end)
