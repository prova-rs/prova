--- Unknown MODULE options are refused, not dropped
--- (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
---
--- The same disease as unit opts, one layer over. Every module namespace parses its options by key
--- lookup, so a typo'd or version-mismatched key reads as CONFIGURED: the run behaves as though the
--- option were absent, and nothing anywhere says so. Two field cases, both real:
---
---   * `docker.build{ first_byte = … }` against a prova built without the option parsed clean and
---     proved nothing about the bound it named — the version-skew half, and why refusing is what a
---     proof written for a newer prova NEEDS from an older one.
---   * `shell.spawn("kubectl", { args = {…} })` started a bare `kubectl` that printed usage into a
---     discarded stdout. `spawn` is the worst possible host, because the process still STARTS: the
---     run failed minutes later waiting for an effect nobody had ever asked for.
---
--- Every case drives the SUBJECT through `prova.bin` — a refusal proven against the CONDUCTING
--- binary would prove whatever prova happens to be installed, which is exactly the mistake that
--- let the original defect through.

--- Run one snippet in the subject's full runtime. `check` is deliberately off: these cases are
--- ABOUT the non-zero exit, and the message lands on stderr.
local function evaluated(code)
  return shell.run({ prova.bin, "eval", code }, { merge_stderr = true, timeout = "60s" })
end

--- Every gated surface, each with a key that is plausibly right and actually wrong. The nested
--- `wait` table earns its own row: a typo INSIDE it is the more dangerous of the two, because
--- `wait = { prot = 5432 }` is a readiness contract that waits for nothing, so the container is
--- handed over unready and the failure surfaces somewhere else entirely.
local SURFACES = {
  { site = "shell.run", key = "tiemout", meant = "timeout",
    code = 'shell.run("echo hi", { tiemout = "10s" })' },
  { site = "shell.spawn", key = "envs", meant = "env",
    code = 'shell.spawn("echo hi", { envs = { A = "1" } })' },
  { site = "docker.build", key = "frist_byte", meant = "first_byte",
    code = 'docker.build{ context = ".", frist_byte = "90s" }' },
  { site = "docker.run", key = "imgae", meant = "image",
    code = 'docker.run{ image = "busybox", imgae = "busybox" }' },
  { site = "docker.run `wait`", key = "prot",
    code = 'docker.run{ image = "busybox", wait = { prot = 5432 } }' },
  { site = "http request options", key = "jsno",
    code = 'http.get("http://127.0.0.1:1/", { jsno = {} })' },
  { site = "http.client", key = "timeuot", meant = "timeout",
    code = 'http.client{ base_url = "http://127.0.0.1:1", timeuot = "5s" }' },
  { site = "http.wait_for", key = "stauts", meant = "status",
    code = 'http.wait_for("http://127.0.0.1:1/", { stauts = 204 })' },
  -- The double/proxy constructors. `ctx` is deliberately nil: the gate runs BEFORE the context
  -- check, so these need no runtime and no daemon, and a row that started failing on `ctx`
  -- instead would be telling us the gate had moved behind it.
  { site = "http.mock", key = "netwrok", meant = "network",
    code = 'http.mock(nil, { netwrok = true })' },
  { site = "http.proxy", key = "upstram", meant = "upstream",
    code = 'http.proxy(nil, { upstram = "http://127.0.0.1:1" })' },
  { site = "grpc.mock", key = "protos", meant = "proto",
    code = 'grpc.mock(nil, { protos = "x.proto" })' },
  { site = "grpc.proxy", key = "casette", meant = "cassette",
    code = 'grpc.proxy(nil, { upstream = "http://127.0.0.1:1", casette = "c" })' },
  { site = "socket.mock", key = "adr", meant = "addr",
    code = 'socket.mock(nil, { framing = "line", adr = "tcp://127.0.0.1:0" })' },
  { site = "socket.listen", key = "adr", meant = "addr",
    code = 'socket.listen(nil, { framing = "line", adr = "tcp://127.0.0.1:0" })' },
  { site = "socket.proxy", key = "framng", meant = "framing",
    code = 'socket.proxy(nil, { upstream = "tcp://127.0.0.1:1", framng = "line" })' },
  { site = "websocket.proxy", key = "upstrem", meant = "upstream",
    code = 'websocket.proxy(nil, { upstrem = "ws://127.0.0.1:1" })' },
  { site = "terminal.mock", key = "az", meant = "as",
    code = 'terminal.mock(nil, { az = "git" })' },
  { site = "terminal.proxy", key = "mods", meant = "mode",
    code = 'terminal.proxy(nil, { as = "git", mods = "record" })' },
  { site = "shell.proxy", key = "redct", meant = "redact",
    code = 'shell.proxy(nil, { as = "git", redct = {} })' },
}

--- `websocket.mock` reads NO options at all, so its closed set is empty — anything passed was
--- dropped whole before this. Its own row, because "accepted: " with nothing after it is a
--- different shape of message and worth seeing rendered.
local HONORS_NOTHING = {
  { site = "websocket.mock", key = "anything",
    code = 'websocket.mock(nil, { anything = 1 })' },
}

prova.test("a typo'd module option is refused at every surface that takes one", {
  covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
  proves = "one guarded door is a false sense of a closed house — the gate is worth having only if a proof cannot find an ungated way in, so every surface that parses an opts table is asserted here rather than the one the defect was found on",
}, function(t)
  for _, case in ipairs(SURFACES) do
    local r = evaluated(case.code)
    t:expect(r.code, case.site .. " refuses"):never():equals(0)
    t:expect(r.stdout, case.site .. " names the offending key"):contains(case.key)
    t:expect(r.stdout, case.site .. " names itself, so the fix is one jump"):contains(case.site)
    -- The accepted set is always listed; a suggestion rides along only when the key is close
    -- enough that naming one is not a guess (crates/prova-core/src/suggest.rs).
    if case.meant then
      t:expect(r.stdout, case.site .. " names the spelling meant"):contains(case.meant)
    end
  end
end)

prova.test("`args` teaches the argv form rather than merely denying itself", {
  covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
  proves = "`args` is what every other process API in the world takes, so nearest-spelling has nothing to offer against `{cwd, env}` — a bare 'unknown option `args`' would be true and useless, and the author needs the argv table that already does the job",
}, function(t)
  for _, verb in ipairs({ "run", "spawn" }) do
    local r = evaluated('shell.' .. verb .. '("kubectl", { args = { "get", "pods" } })')
    t:expect(r.code, verb .. " refuses"):never():equals(0)
    t:expect(r.stdout, verb .. " names the argv table, not just the key"):contains("argv")
    t:expect(r.stdout, verb .. " shows the shape to write"):contains('"kubectl", "get", "pods"')
  end
end)

prova.test("a positional entry is refused too — it is the same drop wearing a different shape", {
  covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
  proves = "`{ \"--flag\" }` looks like arguments to the author and is nothing to prova; catching only STRING keys would leave the shape that reads most like a command line silently discarded",
}, function(t)
  local r = evaluated('shell.run("echo hi", { "--quiet" })')
  t:expect(r.code, "the positional entry is refused"):never():equals(0)
  t:expect(r.stdout, "…and named as positional"):contains("positional")
end)

--- The negative control that makes every refusal above meaningful. A closed set is only safe if it
--- is COMPLETE: a gate that forgets one real spelling turns a working suite red at upgrade, which
--- is a worse failure than the one it was built to prevent.
---
--- Asserting on the ERROR TEXT rather than on success is what lets the docker rows run on a host
--- with no daemon: `docker.run{…}` there fails at the socket, which is a different and perfectly
--- acceptable failure. The assertion is that it never fails at the GATE.
local ACCEPTED = {
  { site = "shell.run", code = [[
shell.run("echo hi", {
  cwd = ".", env = { A = "1" }, timeout = "30s", idle_timeout = "20s",
  first_byte = "10s", check = false, merge_stderr = true, stdin = "",
})]] },
  { site = "shell.spawn", code = 'shell.spawn("echo hi", { cwd = ".", env = { A = "1" } })' },
  { site = "docker.build", code = [[
docker.build{
  context = ".", dockerfile = "Dockerfile", tag = "t:1", buildargs = { A = "1" },
  secrets = {}, target = "s", pull = false, nocache = false, first_byte = "90s",
}]] },
  { site = "docker.run", code = [[
docker.run{
  image = "busybox", ports = { 8080 }, env = { A = "1" }, command = { "true" },
  network = "n", alias = "a", extra_hosts = {},
  wait = { port = 8080, timeout = "30s", every = "250ms" },
}]] },
  { site = "http request options", code = [[
http.get("http://127.0.0.1:1/", {
  headers = { ["x-a"] = "1" }, json = { a = 1 }, timeout = "1s", redirects = false,
})]] },
  { site = "http.client", code =
    'http.client{ base_url = "http://127.0.0.1:1", headers = { ["x-a"] = "1" }, timeout = "1s" }' },
  { site = "http.wait_for", code =
    'http.wait_for("http://127.0.0.1:1/", { status = 204, headers = { ["x-a"] = "1" }, timeout = "10ms", every = "5ms" })' },
  -- The doubles' accepted sets. Each fails on the missing `ctx`, which is a DIFFERENT and
  -- perfectly good failure — the assertion below is only that it never fails at the gate.
  { site = "http.mock", code =
    'http.mock(nil, { network = true, passthrough = "http://x", record = "c.yaml", allow_handler_errors = true, redact = { "authorization" } })' },
  { site = "http.proxy", code =
    'http.proxy(nil, { mode = "record", upstream = "http://x", cassette = "c.yaml", redact = {} })' },
  { site = "grpc.proxy", code =
    'grpc.proxy(nil, { mode = "replay", upstream = "http://x", cassette = "c.yaml", network = true, redact = {} })' },
  { site = "socket.proxy", code =
    'socket.proxy(nil, { addr = "tcp://127.0.0.1:0", upstream = "tcp://127.0.0.1:1", framing = "line", mode = "passthrough", cassette = "c", redact = {} })' },
  { site = "terminal.proxy", code =
    'terminal.proxy(nil, { as = "git", upstream = "/bin/git", mode = "record", cassette = "c" })' },
  { site = "shell.proxy", code =
    'shell.proxy(nil, { as = "git", upstream = "/bin/git", mode = "record", cassette = "c", redact = {} })' },
}

prova.test("every accepted module option still parses — the gate refuses typos, not the API", {
  covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
  proves = "the closed sets are asserted COMPLETE, one surface at a time, by declaring all of each at once: without this the eight refusals above would be satisfied by a gate that accepted nothing at all",
}, function(t)
  for _, case in ipairs(ACCEPTED) do
    local r = evaluated(case.code)
    -- It may well fail (no daemon, nothing listening on port 1) — but never AT THE GATE.
    t:expect(r.stdout, case.site .. " accepts its whole documented set")
      :never():contains("unknown option")
    t:expect(r.stdout, case.site .. " reports no positional confusion")
      :never():contains("positional")
  end
end)

prova.test("the three ways to name a body are mutually exclusive, not silently ranked", {
  covers = "docs/design/agent-ergonomics.md#http-form-and-raw-bodies",
  proves = "an `if json … else if form … else if body` chain sends a request the author did not write and reports the endpoint's honest answer to it, so the debugging starts at the server — refusing is what keeps the call and the request the same thing",
}, function(t)
  local r = evaluated('http.post("http://127.0.0.1:1/", { json = { a = 1 }, form = { b = "2" } })')
  t:expect(r.code, "naming the body twice is refused"):never():equals(0)
  t:expect(r.stdout, "both spellings are named"):contains("json")
  t:expect(r.stdout, "…so the author knows which two collided"):contains("form")
end)

prova.test("a namespace that honors NO options says so, rather than accepting a table it ignores", {
  covers = "docs/design/agent-ergonomics.md#module-opts-silently-ignored",
  proves = "`websocket.mock` never read its opts argument at all, so every key an author passed was dropped whole — the widest possible silent drop, and the one least likely to be suspected, because the call looks configured",
}, function(t)
  for _, case in ipairs(HONORS_NOTHING) do
    local r = evaluated(case.code)
    t:expect(r.code, case.site .. " refuses"):never():equals(0)
    t:expect(r.stdout, case.site .. " names the key it cannot honor"):contains(case.key)
  end
end)
