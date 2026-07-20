-- SOAK — does this container runtime bind the ports it says it exposed, and does the ANSWER DEPEND
-- ON WHO ASKS?
--
-- Chasing an intermittent suite failure produced a measured claim: Docker Desktop, under load,
-- occasionally starts a container whose port map reports `80/tcp=[]` — exposed, bound to nothing,
-- stably, on a running container. prova now replaces such a container on a spaced retry, so a
-- caller never sees it; `docker.diagnostics()` is how anyone learns it happened.
--
-- But "the runtime did it" was an inference, not a measurement. Every observation so far came
-- through prova, which means a bug of our own would look identical: a method that quietly returns a
-- default, a stale value read before it is written, a race between our create and our inspect, a
-- client library disagreeing with the daemon about API versions. A defect we cause and a defect we
-- suffer produce the same error message.
--
-- So the soak is a 2x2. Two CLIENTS (prova's bollard client, and the `docker` CLI shelling out) x
-- two RUNTIMES (whatever DOCKER_HOST points at, run twice):
--
--                     Docker Desktop        OrbStack
--     prova/bollard   [ defects? ]          [ defects? ]
--     docker CLI      [ defects? ]          [ defects? ]
--
-- and the shape of the result names the culprit:
--
--   * defects in the prova row ONLY            -> the bug is OURS. Bollard usage, our polling, our
--                                                 concurrency — the daemon is fine and we are not.
--   * defects in the Docker Desktop column ONLY -> the bug is the RUNTIME's, and recovery is the
--                                                 right response. Make it robust, keep measuring.
--   * defects in both rows, one column          -> runtime defect, and prova is merely the messenger.
--   * defects everywhere                        -> something environmental (kernel, VM, host load).
--   * defects nowhere                           -> the trigger is not reproduced; do not conclude.
--
-- FAIRNESS is the whole game here, because the two arms must differ in exactly one thing: the
-- client. Both create a container publishing one random host port on 127.0.0.1, start it, then poll
-- the port map on the SAME 2s budget at the SAME 50ms interval, classify by the SAME rule (a host
-- port present = published; the key present with nothing bound = the defect; the key absent = still
-- being wired), and tear down. The CLI arm deliberately reimplements prova's protocol rather than
-- calling `docker run`, so a difference in result cannot be a difference in method. It also does NOT
-- retry: it measures raw defect occurrence, which is what prova's `port_bind_recoveries` counts too,
-- so the two rows are counting the same event.
--
-- WHAT THE MATRIX FOUND, and it was not what the earlier numbers suggested.
--
-- Before the CLI arm existed, every observation came through prova, and the readings looked damning
-- for one runtime: ~2400 concurrent starts on Docker Desktop produced 7 "defects" (1, 2, 4 across
-- three rounds) while OrbStack produced none. Concurrency, not volume, was the trigger — 1200
-- sequential starts found nothing at all.
--
-- Then the same runtime, same concurrency, same 800 starts, one variable changed:
--
--                          Docker Desktop, 8 workers x 100
--     prova/bollard                    7 defects
--     docker CLI (same protocol)       0 defects        <- the tell
--
-- A defect that only one client can see is not the daemon's. The cause was ours: the containers ran
-- `sleep 2`, the port scan budget is 2s, and a container that has EXITED has its bindings cleared by
-- the daemon. prova read "port requested, nothing bound", called it a runtime defect, and recreated
-- a container that had simply finished its job. Confirmed by holding everything else fixed:
--
--     container lifetime 2s   ->  7 defects
--     container lifetime 30s  ->  0 defects
--     lifetime 2s, after fix  ->  0 defects   (and 20s faster, for want of pointless recreates)
--
-- Two lessons worth keeping. First, an instrument that can only observe through one client cannot
-- tell "the system misbehaved" from "we misread it" — the second arm is what made the difference
-- visible, and it took one run. Second, the counters were confidently wrong: they attributed our
-- misreading to a specific product, and the number being small and plausible is exactly what made
-- it credible.
--
-- The recovery machinery is still right and still tested (see `modules::docker::tests`), but as of
-- this writing NO confirmed instance of the runtime defect it was built for has been observed on
-- either runtime. Treat it as an unproven contingency, not as a known-necessary workaround.
--
-- All numbers are a snapshot of one machine and two daemon versions. Re-measure before repeating
-- them; that is the entire point of keeping this runnable.
--
-- GATING. `soak` (opt-in, PROVA_SOAK) and `docker` (present). Absent either, this skips.
--
--   PROVA_SOAK=1 PROVA_SOAK_WORKERS=8 prova -j 8 -k soak            # both arms, current runtime
--   PROVA_SOAK=1 PROVA_SOAK_CLIENT=cli prova -j 8 -k soak           # one arm only
--   PROVA_SOAK=1 DOCKER_HOST=unix://$HOME/.orbstack/run/docker.sock prova -j 8 -k soak
--
-- Select with `-k`, not by path. An explicit path bypasses the manifest, and the companion that
-- registers `soak` goes with it — so `prova proofs/soak` skips this every time, for a reason that
-- has nothing to do with whether a soak was asked for.
--
-- On DOCKER_HOST: it is the only way to aim this at a specific runtime, for BOTH arms. `docker
-- context use` moves the CLI and not prova — bollard reads DOCKER_HOST or the default socket and
-- knows nothing about contexts — so a context switch would point the two arms at different daemons
-- and silently compare a runtime against itself. (Verified both ways: with DOCKER_HOST set to each
-- socket in turn, bollard's container appears on that daemon and only that one, and `shell.run`
-- inherits the variable so the CLI arm follows it too.)

local STARTS     = tonumber(os.getenv("PROVA_SOAK_STARTS") or "") or 200
local WORKERS    = tonumber(os.getenv("PROVA_SOAK_WORKERS") or "") or 1
local CLIENT     = os.getenv("PROVA_SOAK_CLIENT") or "both"
local PER_WORKER = math.max(1, math.floor(STARTS / WORKERS))

-- The workload is parameterised so this can be aimed at the case under suspicion, not just at a
-- convenient one. The original production failures were `moul/grpcbin` on 9000 — an amd64-only
-- image running emulated on arm64, and a long-lived server rather than a `sleep` — which is a very
-- different animal from a native alpine that exits on cue.
local IMAGE      = os.getenv("PROVA_SOAK_IMAGE") or "alpine:3.20"
local PORT       = tonumber(os.getenv("PROVA_SOAK_PORT") or "") or 80
-- How long the container lives. A VARIABLE, not a detail: a container that has exited has its port
-- bindings cleared by the daemon, so a lifetime near the scan budget lets "finished normally"
-- masquerade as "never bound" — which is exactly how we once blamed a runtime for our own bug.
-- Set to "none" to run the image's own entrypoint untouched (a server that never exits).
local LIFETIME   = os.getenv("PROVA_SOAK_LIFETIME") or "2"
local RUN_FOREVER = LIFETIME == "none"
-- Whether the prova arm also waits for readiness, as a real resource does. The original failure
-- surfaced AFTER readiness had passed — the server was listening inside the container while no host
-- mapping existed — so reproducing it faithfully means asking for the wait.
local WITH_WAIT  = os.getenv("PROVA_SOAK_WAIT") == "1"
-- Mirrors prova's own resolution budget (`published_ports`), so neither arm is given more patience
-- than the other.
local BUDGET_MS  = 2000
local EVERY_MS   = 50

-- Classify a `docker inspect .NetworkSettings.Ports` payload the way prova classifies the daemon's
-- typed equivalent. Plain substring matching, no patterns: the shapes are unambiguous.
local function classify(json)
  if json:find("HostPort", 1, true) then return "published" end
  if json:find('"' .. PORT .. '/tcp"', 1, true) then return "bound_nothing" end
  return "never_appeared"
end

-- One container start via the `docker` CLI, following prova's protocol step for step.
-- Returns "published" | "bound_nothing" | "never_appeared" | "error:<detail>".
local function cli_start_once()
  local argv = { "docker", "create", "-p", "127.0.0.1::" .. PORT, IMAGE }
  if not RUN_FOREVER then
    argv[#argv + 1] = "sleep"
    argv[#argv + 1] = LIFETIME
  end
  local created = shell.run(argv)
  if created.code ~= 0 then
    return "error:create: " .. tostring(created.stderr)
  end
  local id = tostring(created.stdout):gsub("%s+", "")
  if id == "" then
    return "error:create returned no id"
  end

  local outcome
  local started = shell.run({ "docker", "start", id })
  if started.code ~= 0 then
    outcome = "error:start: " .. tostring(started.stderr)
  else
    -- Poll on prova's budget, deciding only at the deadline — a key that is merely late must not be
    -- mistaken for one that is bound to nothing.
    local last = "never_appeared"
    local waited = 0
    while true do
      local got = shell.run({
        "docker", "inspect", "--format", "{{json .NetworkSettings.Ports}}", id,
      })
      if got.code == 0 then
        last = classify(tostring(got.stdout))
        if last == "published" then break end
      end
      if waited >= BUDGET_MS then break end
      prova.sleep(EVERY_MS)
      waited = waited + EVERY_MS
    end
    outcome = last
  end

  -- Always reap: this arm owns containers prova knows nothing about, so nothing else will.
  shell.run({ "docker", "rm", "-f", id })
  return outcome
end

-- The prova/bollard arm. Reports whether the start was usable, and lets prova's own counters record
-- any defect it healed along the way.
local function prova_start_once()
  local ok, err = pcall(function()
    local spec = { image = IMAGE, ports = { PORT } }
    if not RUN_FOREVER then
      spec.command = { "sleep", LIFETIME }
    end
    if WITH_WAIT then
      spec.wait = { port = PORT, timeout = "60s" }
    end
    local c = docker.run(spec)
    -- Resolving the host port is the operation the defect actually breaks; asking every time is what
    -- turns a bad binding into an observable event rather than a latent one.
    local hp = c:host_port(PORT)
    c:stop()
    return hp
  end)
  if ok then return "published" end
  return "error:" .. tostring(err)
end

local function want(client)
  return CLIENT == "both" or CLIENT == client
end

-- One test per worker per arm, so prova's scheduler (`-j N`) drives simultaneous creates — the shape
-- under which the defect appears at all.
for w = 1, WORKERS do
  if want("prova") then
    prova.test(string.format("soak [prova/bollard] worker %d/%d: %d starts", w, WORKERS, PER_WORKER),
               { requires = { "soak", "docker" } }, function(t)
      local before = docker.diagnostics()
      local usable, errors = 0, {}

      for i = 1, PER_WORKER do
        local outcome = prova_start_once()
        if outcome == "published" then
          usable = usable + 1
        else
          errors[#errors + 1] = string.format("start %d: %s", i, outcome)
        end
      end

      local after = docker.diagnostics()
      -- Counters are process-wide, so with several workers these deltas overlap; the per-worker
      -- `usable` count stays exact regardless, and the headline question — did this runtime
      -- misbehave at all — is answered either way.
      local recovered = after.port_bind_recoveries - before.port_bind_recoveries
      local gave_up   = after.port_bind_failures   - before.port_bind_failures

      print(string.format("soak[prova] w%d: %d/%d usable | bind defects seen: %d recovered, %d unrecoverable",
                          w, usable, PER_WORKER, recovered, gave_up))
      for _, e in ipairs(errors) do print("soak[prova]: " .. e) end

      -- The contract, not the runtime's report card: a start either produced a usable container or
      -- raised. Silent success with an unusable container is the outcome that must never happen.
      t:expect(usable, "every start yielded a usable container or raised"):equals(PER_WORKER)
      t:expect(gave_up, "no unrecoverable bind failures"):equals(0)
    end)
  end

  if want("cli") then
    prova.test(string.format("soak [docker CLI] worker %d/%d: %d starts", w, WORKERS, PER_WORKER),
               { requires = { "soak", "docker" } }, function(t)
      local published, bound_nothing, never, errors = 0, 0, 0, {}

      for i = 1, PER_WORKER do
        local outcome = cli_start_once()
        if outcome == "published" then
          published = published + 1
        elseif outcome == "bound_nothing" then
          bound_nothing = bound_nothing + 1
        elseif outcome == "never_appeared" then
          never = never + 1
        else
          errors[#errors + 1] = string.format("start %d: %s", i, outcome)
        end
      end

      print(string.format("soak[cli]   w%d: %d/%d published | bound_nothing=%d never_appeared=%d errors=%d",
                          w, published, PER_WORKER, bound_nothing, never, #errors))
      for _, e in ipairs(errors) do print("soak[cli]: " .. e) end

      -- This arm is a MEASUREMENT, not a contract: the CLI has no recovery, so a defect here is a
      -- fact about the runtime rather than a bug in prova, and failing on it would make the soak
      -- unrunnable exactly when it has something to say. Only genuine breakage fails the arm.
      t:expect(#errors, "no create/start errors"):equals(0)
      t:expect(published + bound_nothing + never, "every start was classified"):equals(PER_WORKER)
    end)
  end
end
