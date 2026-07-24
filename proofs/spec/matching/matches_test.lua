--- `:matches(shape)` — ONE structural-subset semantics behind three surfaces (t:expect,
--- double:on, mock stub matchers). Polymorphic by argument, following the `contains` precedent:
--- a string is a Lua pattern (shipped today), a table is a recursive subset.

-- A realistic subject: the k8s-shaped payload that motivated the matcher.
local deploy = {
  kind = "Deployment",
  metadata = { name = "my-app", labels = { app = "my-app", tier = "web" } },
  status = {
    readyReplicas = 3,
    conditions = {
      { type = "Available", status = "True" },
      { type = "Progressing", status = "True", reason = "NewReplicaSetAvailable" },
    },
  },
}

prova.test("a table shape is a recursive subset — extra subject keys ignored", function(t)
  t:expect(deploy):matches{ status = { readyReplicas = 3 } }
end)

prova.test("nested shapes recurse to any depth", function(t)
  t:expect(deploy):matches{ metadata = { labels = { app = "my-app" } } }
end)

prova.test("arrays match same-index, recursing into elements", function(t)
  t:expect(deploy):matches{ status = { conditions = { { type = "Available" } } } }
end)

prova.test("a shape array shorter than the subject's is fine", function(t)
  t:expect({ xs = { "a", "b", "c" } }):matches{ xs = { "a" } }
end)

prova.test("a shape array longer than the subject's fails", function(t)
  t:expect({ xs = { 1, 2 } }):never():matches{ xs = { 1, 2, 3 } }
end)

prova.test("a wrong leaf value fails", function(t)
  t:expect(deploy):never():matches{ status = { readyReplicas = 4 } }
end)

prova.test("a key missing from the subject fails", function(t)
  t:expect(deploy):never():matches{ status = { observedGeneration = 7 } }
end)

prova.test("integer and float leaves coerce, exactly like equals", function(t)
  t:expect({ n = 3 }):matches{ n = 3.0 }
end)

prova.test("an empty shape matches any table (the vacuous subset)", function(t)
  t:expect(deploy):matches{}
end)

prova.test("the json.null sentinel is matchable in authored tables", { spec = "blocked on the formats module (json.null)" }, function(t)
  t:expect({ x = json.null }):matches{ x = json.null }
end)

-- Shipped before this suite existed; pinned so the polymorphic contract stays whole.
prova.test("a string shape stays a Lua pattern match", function(t)
  t:expect("prova-0.9.1"):matches("^prova%-%d")
end)
