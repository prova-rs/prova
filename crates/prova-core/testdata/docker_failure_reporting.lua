-- PROOF (failure reporting) — when a container does not come up, `docker.run` must say WHY, and must
-- not make you wait for a timeout to learn something already knowable.
--
-- Both proofs here are about the *diagnosis*, not the happy path. They exist because the original
-- implementation answered every failure the same way — "not ready within <timeout>" — after burning
-- the full budget, whether the container had crashed on startup a second earlier or was genuinely
-- still booting. That error is uninformative and slow in exactly the case where you most need speed
-- and detail, and it is what turned real container failures into "flaky docker" folklore.
--
-- The distinction being proved: a container that has EXITED can never become ready (fail now), while
-- a container that is RUNNING but not yet listening might still make it (keep waiting). Confusing
-- the two in either direction is a bug — fast-failing a slow starter would be far worse than the
-- original behavior.
--
-- Run standalone: prova crates/prova-core/testdata/docker_failure_reporting.lua   (requires docker)

-- Wall clock, deliberately: `os.clock()` measures CPU time, and a process blocked on docker burns
-- almost none — so a duration bound written with it passes no matter how long the wait actually was.
-- `os.time()` is only second-resolution, which is why the bounds below are coarse.
local function now() return os.time() end

prova.test("a container that exits is reported at once, with its exit code and logs",
           { requires = { "docker" } }, function(t)
  local started = now()
  local ok, err = pcall(function()
    docker.run{
      image = "alpine:3.20",
      -- Writes a diagnostic, then dies. Nothing here can ever listen.
      command = { "sh", "-c", "echo 'fatal: config missing' >&2; exit 3" },
      ports = { 8080 },
      -- Deliberately generous. If the implementation waits this out, the proof fails on duration —
      -- which is the point: the budget is for slow starts, not for corpses.
      wait = { port = 8080, timeout = "60s" },
    }
  end)
  local elapsed = now() - started

  t:expect(ok, "docker.run on a dying container"):equals(false)
  local msg = tostring(err)
  -- The exit code is the single most useful fact, and the container's own output is the second.
  t:expect(msg, "names the exit code"):contains("exited with code 3")
  t:expect(msg, "carries the container's own stderr"):contains("fatal: config missing")
  -- Well under the 60s budget. Loose enough not to be a benchmark, tight enough to catch a
  -- regression back to "wait it out".
  t:expect(elapsed < 20, "failed fast rather than waiting out the timeout"):equals(true)
end)

prova.test("a live container that is merely slow is NOT failed early",
           { requires = { "docker" } }, function(t)
  local started = now()
  local ok, err = pcall(function()
    docker.run{
      image = "alpine:3.20",
      -- Healthy and running, but nothing ever binds the port: the shape of a slow starter, held
      -- indefinitely so the outcome is deterministic.
      command = { "sh", "-c", "echo 'starting up'; sleep 120" },
      ports = { 8080 },
      wait = { port = 8080, timeout = "5s" },
    }
  end)
  local elapsed = now() - started

  t:expect(ok, "docker.run on a container that never listens"):equals(false)
  local msg = tostring(err)
  -- A RUNNING container must be given its full budget — the liveness check must not steal it.
  t:expect(msg, "reports a timeout, not a false crash"):contains("not ready within")
  t:expect(msg, "still explains itself with logs"):contains("starting up")
  t:expect(elapsed >= 4, "waited out the budget instead of giving up early"):equals(true)
end)
