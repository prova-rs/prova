-- Suite setup: runs once, in the suite's shared state, before the test files. Declares the suite's
-- shared fixtures. The counters live in globals so the tests can prove build-once semantics.
_G.suite_builds = 0
_G.file_builds = 0

-- Built ONCE for the whole suite, shared live across every file.
prova.fixture("shared", Scope.Suite, function(ctx)
  _G.suite_builds = _G.suite_builds + 1
  return _G.suite_builds
end)

-- Rebuilt once PER FILE within the suite.
prova.fixture("perfile", Scope.File, function(ctx)
  _G.file_builds = _G.file_builds + 1
  return _G.file_builds
end)
