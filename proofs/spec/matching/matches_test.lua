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

prova.test("a table shape is a recursive subset — extra subject keys ignored", { spec = false }, function(t)
  t:expect(deploy):matches{ status = { readyReplicas = 3 } }
end)

prova.test("nested shapes recurse to any depth", { spec = false }, function(t)
  t:expect(deploy):matches{ metadata = { labels = { app = "my-app" } } }
end)

prova.test("arrays match same-index, recursing into elements", { spec = false }, function(t)
  t:expect(deploy):matches{ status = { conditions = { { type = "Available" } } } }
end)

prova.test("a shape array shorter than the subject's is fine", { spec = false }, function(t)
  t:expect({ xs = { "a", "b", "c" } }):matches{ xs = { "a" } }
end)

prova.test("a shape array longer than the subject's fails", { spec = false }, function(t)
  t:expect({ xs = { 1, 2 } }):never():matches{ xs = { 1, 2, 3 } }
end)

prova.test("a wrong leaf value fails", { spec = false }, function(t)
  t:expect(deploy):never():matches{ status = { readyReplicas = 4 } }
end)

prova.test("a key missing from the subject fails", { spec = false }, function(t)
  t:expect(deploy):never():matches{ status = { observedGeneration = 7 } }
end)

prova.test("integer and float leaves coerce, exactly like equals", { spec = false }, function(t)
  t:expect({ n = 3 }):matches{ n = 3.0 }
end)

prova.test("an empty shape matches any table (the vacuous subset)", { spec = false }, function(t)
  t:expect(deploy):matches{}
end)

prova.test("the json.null sentinel is matchable in authored tables", function(t)
  t:expect({ x = json.null }):matches{ x = json.null }
end)

-- Shipped today; pinned here so the polymorphic contract stays whole. Born graduated.
prova.test("a string shape stays a Lua pattern match", { spec = false }, function(t)
  t:expect("prova-0.9.1"):matches("^prova%-%d")
end)
