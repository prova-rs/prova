-- The mock network vantage — C2's *mechanism*, everything provable without real Linux semantics.
--
-- The end-to-end claim ("a containerized SUT reaches a host-bound mock, and 127.0.0.1 would NOT
-- work") can only be proved on native Linux — on Docker Desktop a loopback bind is reachable, so the
-- mutation check cannot fail here. That proof lives in the Parallels harness (requires "parallels").
-- What IS provable on any host is that the wiring is correct: the vantage appears only when asked,
-- reports the host-gateway address, the mock actually binds beyond loopback, and the SUT container
-- carries the extra_hosts entry that makes the name resolve. If any of those is wrong, the Linux
-- proof cannot even get off the ground — so proving them here is the cheap precondition.

-- The machine's own routable (non-loopback) IPv4, macOS or Linux, or "" if none. Used to prove the
-- bind changed: a 0.0.0.0 server answers here, a 127.0.0.1 one does not.
local function routable_ip()
  local r = shell.run([[
    if command -v ip >/dev/null 2>&1; then
      ip route get 1.1.1.1 2>/dev/null | sed -n 's/.*src \([0-9.]*\).*/\1/p'
    else
      ipconfig getifaddr "$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')" 2>/dev/null
    fi
  ]])
  return (r.stdout or ""):gsub("%s+$", "")
end

prova.test("no vantage by default — loopback only", function(t)
  local m = http.mock(t)
  t:expect(m.network):is_nil()          -- not exposed unless asked
  t:expect(m.url):contains("127.0.0.1") -- own-process probe address, unchanged
end)

prova.test("network = true exposes a host-gateway vantage", function(t)
  local m = http.mock(t, { network = true })
  t:expect(m.network):never():is_nil()
  t:expect(m.network.host):equals("host.docker.internal")
  t:expect(m.network.port):equals(m.port)
  t:expect(m.network.url):equals("http://host.docker.internal:" .. m.port)
  -- `.url` is still loopback — that is how the TEST reaches the mock; `.network.url` is how a
  -- container does. Two vantages, same server.
  t:expect(m.url):equals("http://127.0.0.1:" .. m.port)
end)

prova.test("a string overrides the host name for another substrate", function(t)
  local m = http.mock(t, { network = "gateway.local" })
  t:expect(m.network.host):equals("gateway.local")
  t:expect(m.network.url):equals("http://gateway.local:" .. m.port)
end)

-- The bind actually changed: with network on, the mock answers on a non-loopback interface. Proven
-- by reaching it via the machine's own routable address, not 127.0.0.1. (Skipped if the host has no
-- non-loopback IPv4 — a CI container sometimes doesn't.)
prova.test("network = true binds beyond loopback", function(t)
  local m = http.mock(t, { network = true })
  m:on{ path = "/ping" }:reply{ status = 200, body = "pong" }

  local addr = routable_ip()
  if addr == "" or addr == "127.0.0.1" then
    t:skip("no non-loopback IPv4 to probe")
    return
  end
  local res = http.get("http://" .. addr .. ":" .. m.port .. "/ping")
  t:expect(res.status):equals(200)
  t:expect(res.body):equals("pong")
end)

prova.test("a default mock stays loopback-only", function(t)
  local m = http.mock(t)
  m:on{ path = "/ping" }:reply{ status = 200 }
  local addr = routable_ip()
  if addr == "" or addr == "127.0.0.1" then
    t:skip("no non-loopback IPv4 to probe")
    return
  end
  -- A short timeout: a loopback-only server does not answer on the routable address.
  local ok = pcall(function()
    http.get("http://" .. addr .. ":" .. m.port .. "/ping", { timeout = "1s" })
  end)
  t:expect(ok):is_false()
end)

prova.test("grpc.mock exposes the same vantage", function(t)
  local proto = t:tempdir() .. "/p.proto"
  fs.write(proto, "syntax=\"proto3\"; package p; service S { rpc Go (E) returns (E); } message E {}")
  local m = grpc.mock(t, { proto = proto, network = true })
  t:expect(m.network.host):equals("host.docker.internal")
  t:expect(m.network.url):equals("http://host.docker.internal:" .. m.port)
end)

-- The container half: `docker.run{ extra_hosts }` must actually land the mapping in the container's
-- /etc/hosts, or the SUT cannot resolve the name no matter how the mock binds. Docker-gated; runs on
-- any Docker (the entry is honored identically on Desktop and Linux — only the *gateway* differs).
prova.test("docker.run threads extra_hosts into the container", { requires = { "docker" } }, function(t)
  local c = t:manage(docker.run{
    image = "alpine:3",
    command = { "sleep", "30" },
    extra_hosts = { "host.docker.internal:host-gateway", "example.test:1.2.3.4" },
  })
  -- The literal mapping is unambiguous; assert on it rather than host-gateway (whose resolved IP is
  -- platform-specific — that is exactly what the Linux proof exists to pin down).
  local hosts = c:run({ "getent", "hosts", "example.test" })
  t:expect(hosts):contains("1.2.3.4")
end)
