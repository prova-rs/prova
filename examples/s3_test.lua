--- The `s3` object-storage client + `s3.container` recipe against a REAL MinIO (S3-compatible) in an
--- ephemeral container. Run from the repo root: `prova examples/s3_test.lua`. Requires docker.

local store = prova.fixture("s3", "file", function(ctx)
  return s3.container(ctx).bucket
end)

prova.group("s3", { requires = { "docker" } }, function(g)
  g:test("put / get round-trips an object", function(t)
    local b = t:use(store)
    b:put("greeting.txt", "hello world")
    t:expect(b:get("greeting.txt")):equals("hello world")
  end)

  g:test("exists, list, and delete", function(t)
    local b = t:use(store)
    b:put("a/one.txt", "1")
    b:put("a/two.txt", "2")
    t:expect(b:exists("a/one.txt")):is_true()
    t:expect(b:exists("a/missing.txt")):is_false()
    local keys = b:list("a/")
    t:expect(#keys):equals(2)
    t:expect(keys):contains("a/one.txt")
    b:delete("a/one.txt")
    t:expect(b:exists("a/one.txt")):is_false()
  end)
end)
