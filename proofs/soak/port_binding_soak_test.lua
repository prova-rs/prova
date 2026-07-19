-- SOAK — does this container runtime bind the ports it says it exposed?
--
-- Chasing an intermittent suite failure produced a measured claim: Docker Desktop, under load,
-- occasionally starts a container whose port map reports `5432/tcp=[]` — exposed, bound to nothing,
-- stably, on a running container. Roughly one start in 750. prova now replaces such a container on
-- a spaced retry, so a caller never sees it; `docker.diagnostics()` is how anyone finds out it
-- happened.
--
-- This soak turns that anecdote into a number, per runtime. It is deliberately NOT a pass/fail
-- verdict on the runtime — one bad start in 750 does not make a runtime "broken", and failing the
-- suite over it would misreport severity. What it asserts is that prova's contract holds under
-- sustained churn: every start either yields a usable container or fails loudly, and any
-- misbehaviour is COUNTED rather than silently absorbed. The counts are the deliverable; the
-- assertions guard the contract.
--
-- CONCURRENCY IS THE TRIGGER, not volume. Measured on one machine (M-series macOS):
--
--   sequential, 600 starts   Docker Desktop  0 defects      OrbStack  0 defects
--   8 workers, 800 starts    Docker Desktop  1, 2, 4        OrbStack  0, 0, 0
--                            (three rounds each; every Desktop defect was healed,
--                             none unrecoverable, 800/800 starts usable throughout)
--
-- So 1200 sequential starts found nothing and ~2400 concurrent starts found seven. Depth alone was
-- answering a question nobody asked; `workers` exists to reproduce the conditions. And OrbStack has
-- not reproduced it at all under the same load, which is what makes this look like a Docker Desktop
-- port-plumbing defect rather than something prova provokes.
--
-- Those numbers are a snapshot of one machine and two daemon versions, not a verdict on either
-- product. Re-measure before repeating them; that is the entire point of keeping this runnable.
--
-- GATING. `soak` (opt-in, PROVA_SOAK) and `docker` (present). Absent either, this skips.
--
--   PROVA_SOAK=1 prova -k soak                                     # default runtime, 1 worker
--   PROVA_SOAK=1 PROVA_SOAK_WORKERS=8 prova -j 8 -k soak           # parallel load
--   PROVA_SOAK=1 PROVA_SOAK_STARTS=2000 prova -k soak              # a real overnight run
--   PROVA_SOAK=1 DOCKER_HOST=unix://$HOME/.orbstack/run/docker.sock prova -k soak
--
-- Select with `-k`, not by path. An explicit path bypasses the manifest, and the companion that
-- registers `soak` goes with it — so `prova proofs/soak` skips this every time, for a reason that
-- has nothing to do with whether a soak was asked for.
--
-- On DOCKER_HOST: it is the only way to aim this at a specific runtime. `docker context use` moves
-- the CLI and NOT prova — bollard reads DOCKER_HOST or the default socket and knows nothing about
-- contexts — so a context switch would silently soak the same runtime twice and report agreement
-- that was never measured. (Verified: with DOCKER_HOST set to each socket in turn, the container
-- appears on that daemon and only that one.)

local STARTS  = tonumber(os.getenv("PROVA_SOAK_STARTS") or "") or 200
local WORKERS = tonumber(os.getenv("PROVA_SOAK_WORKERS") or "") or 1
local PER_WORKER = math.max(1, math.floor(STARTS / WORKERS))

-- One test per worker, so prova's own scheduler (`-j N`) runs them concurrently and the daemon sees
-- simultaneous creates — the shape under which the defect was originally observed.
for w = 1, WORKERS do
  prova.test(string.format("soak worker %d/%d: %d starts, each usable or loud", w, WORKERS, PER_WORKER),
             { requires = { "soak", "docker" } }, function(t)
    local before = docker.diagnostics()
    local usable, failures = 0, {}

    for i = 1, PER_WORKER do
      local ok, err = pcall(function()
        local c = docker.run{
          image = "alpine:3.20",
          command = { "sleep", "2" },
          ports = { 80 },
        }
        -- Ask for the host port every time. This is the operation the defect actually breaks, and
        -- resolving it turns a bad binding into an observable event rather than a latent one.
        local hp = c:host_port(80)
        c:stop()
        return hp
      end)
      if ok then
        usable = usable + 1
      else
        failures[#failures + 1] = string.format("worker %d start %d: %s", w, i, tostring(err))
      end
    end

    local after = docker.diagnostics()
    -- Counters are process-wide, so with several workers these deltas overlap. That is fine for the
    -- headline question — did this runtime misbehave at all, and did anything go unhealed — and the
    -- per-worker `usable` count stays exact regardless.
    local recovered = after.port_bind_recoveries - before.port_bind_recoveries
    local gave_up   = after.port_bind_failures   - before.port_bind_failures

    print(string.format("soak w%d: %d/%d usable | bind defects seen: %d recovered, %d unrecoverable",
                        w, usable, PER_WORKER, recovered, gave_up))
    for _, f in ipairs(failures) do print("soak: " .. f) end

    -- The contract, not the runtime's report card: a start either produced a usable container or
    -- raised. Silent success with an unusable container is the one outcome that must never happen.
    t:expect(usable, "every start yielded a usable container or raised"):equals(PER_WORKER)

    -- Anything prova could not heal must have been counted, so a green soak is green for a readable
    -- reason rather than by luck.
    t:expect(gave_up, "no unrecoverable bind failures"):equals(0)
  end)
end
