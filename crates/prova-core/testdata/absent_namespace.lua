-- In a build WITHOUT the redis feature, `redis` is an absent-namespace stub: accessing a field
-- raises a clear "not compiled into this build" error (not a bare nil-index). The raised value is an
-- mlua error object, so match its `tostring`.
prova.test("absent namespace raises a clear error", function(t)
  local ok, err = pcall(function() return redis.client("redis://x") end)
  t:expect(ok):is_false()
  t:expect(tostring(err)):matches("not compiled into this build")
  t:expect(tostring(err)):matches("requires")
end)
