-- C2 end-to-end: a containerized SUT reaches a HOST-bound mock — the proof that only means
-- something on native Linux.
--
-- Written to run INSIDE a Linux host (the Parallels VM, or CI), where prova, the Docker daemon, and
-- the container share one kernel. That colocation is the point: `host-gateway` resolves to the bridge
-- address the host is actually reachable at, and a mock bound to 127.0.0.1 is NOT on that interface.
-- On Docker Desktop for Mac none of this holds — a loopback bind is reachable through the VM proxy,
-- so the negative test below would FAIL there. That divergence is not a flaw; it is why the proof has
-- to run here.
--
-- The "SUT" is a plain alpine container we exec busybox `wget` inside — the smallest real thing that
-- exercises the container→host path. `alpine` (unlike `curlimages/curl`, whose ENTRYPOINT is `curl`)
-- takes a `sleep` command cleanly, so it stays up to be driven black-box.

local SUT = "alpine:3"

-- THE positive proof: mock exposed (0.0.0.0 + host-gateway vantage), the container reaches it.
prova.test("a containerized SUT reaches a host-bound mock via the network vantage",
  { requires = { "docker" } }, function(t)
  local net = t:manage(docker.network())

  local mock = http.mock(t, { network = true })
  mock:on{ path = "/price" }:reply{ status = 200, json = { cents = 999 } }

  -- A container on the topology network, carrying the host-gateway mapping prova.containerized adds.
  local sut = t:manage(docker.run{
    image = SUT, network = net,
    extra_hosts = { "host.docker.internal:host-gateway" },
    command = { "sleep", "300" },
  })

  -- Drive it: wget the mock's NETWORK vantage from inside the container. Returns stdout; raises on a
  -- non-zero exit, so a failure to reach the host would fail the test here.
  local body = sut:run({ "wget", "-qO-", "-T", "5", mock.network.url .. "/price" })
  t:expect(body):contains("999")

  -- Black-box confirmation: the mock recorded the container's call.
  t:expect(mock:received{ path = "/price" }):has_length(1)
end)

-- THE mutation, as a passing negative: a loopback-bound mock is UNREACHABLE from the container. On
-- Docker Desktop this would fail (the SUT would reach it); that it passes here is what proves the
-- vantage is load-bearing, not incidental.
prova.test("a loopback-bound mock is NOT reachable from the container (the mutation)",
  { requires = { "docker" } }, function(t)
  -- The premise — "a container cannot reach the host's loopback" — is a fact about the *environment*,
  -- not the code. It holds on native Linux and is FALSE on Docker Desktop, whose proxy forwards
  -- `host.docker.internal` to the host's 127.0.0.1. So on Desktop this is unanswerable, and per
  -- `test-topology.md` an unanswerable question is a skip, not a failure. (This divergence is the
  -- proof: run it on both and the platforms disagree exactly where C2 says they should.)
  local info = shell.run({ "docker", "info", "--format", "{{.OperatingSystem}}" })
  if (info.stdout or ""):find("Docker Desktop") then
    t:skip("Docker Desktop reaches host loopback; the mutation only holds on native Linux")
    return
  end

  local net = t:manage(docker.network())

  local mock = http.mock(t) -- no `network` → 127.0.0.1 only
  mock:on{ path = "/price" }:reply{ status = 200, json = { cents = 999 } }

  local sut = t:manage(docker.run{
    image = SUT, network = net,
    extra_hosts = { "host.docker.internal:host-gateway" },
    command = { "sleep", "300" },
  })

  -- Point the SUT at the address the vantage WOULD have produced. The mock is not listening there, so
  -- the fetch fails. Wrapped so the non-zero exit becomes a word we can assert on rather than a raise.
  local url = "http://host.docker.internal:" .. mock.port .. "/price"
  local out = sut:run({ "sh", "-c", "wget -qO- -T 5 " .. url .. " && echo REACHED || echo REFUSED" })
  t:expect(out):contains("REFUSED")
  t:expect(out):never():contains("REACHED")
  t:expect(mock:received()):has_length(0) -- nothing arrived
end)
