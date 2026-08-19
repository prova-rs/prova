--- The `stdio` kernel transport — driving a conversational SUT over its pipes
--- (docs/plans/stdio-transport.md, docs/design/mocks-proxies-drivers.md).
---
--- What these pin, and why each one is here rather than assumed:
---
---   * **A conversation, not a batch.** Turn two's CONTENT comes from turn one's REPLY, which is
---     unreachable by writing a request file and redirecting it in — by construction, not by
---     preference. That is the whole feature: batching a session is a race
---     (`agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut`), and a server free to
---     dispatch concurrently answers the second request before the first has stored anything.
---   * **stderr is not in the frame stream.** A server logging to stderr is normal; folding it
---     into stdout would feed log lines to the JSON decoder as protocol garbage.
---   * **Every read is bounded, and says why it failed.** A silent SUT must report the three
---     facts that separate "never started" / "started and said nothing" / "died on the other
---     stream" — otherwise a wedged server costs an afternoon.
---   * **`where` selects among turns**, so an interleaved notification is skipped rather than
---     mistaken for the reply.
---
--- `sh` is the SUT here on purpose: a real conversational server (an MCP server) is proved in
--- proofs/mcp, and what needs pinning HERE is the transport, with nothing between it and the
--- assertion that could hide a mistake.

--- A stateful line-oriented responder: it REMEMBERS what it was told and answers later turns from
--- that memory, which is what makes the round trip unfakeable by a batch.
local function responder(t)
  local p = t:tempdir() .. "/server.sh"
  fs.write(p, [[
#!/bin/sh
session=""
while IFS= read -r line; do
  case "$line" in
    *open*)  session="S-$(echo "$line" | tr -dc '0-9')"
             echo "{\"id\":1,\"session\":\"$session\"}" ;;
    *use*)   echo "{\"id\":2,\"saw\":\"$session\"}" ;;
    *log*)   echo "starting up, all is well" >&2
             echo "{\"id\":3,\"ok\":true}" ;;
    *noisy*) echo "{\"method\":\"progress\",\"pct\":10}"
             echo "{\"method\":\"progress\",\"pct\":90}"
             echo "{\"id\":4,\"done\":true}" ;;
    *quiet*) sleep 30 ;;
  esac
done
]])
  shell.run({ "chmod", "+x", p })
  return p
end

-- ── the conversation ───────────────────────────────────────────────────────────────────────────

prova.test("a request/response session carries state from turn one into turn two", {
  requires = { "unix" },
  covers = "docs/design/agent-ergonomics.md#stdio-cannot-drive-a-conversational-sut",
  proves = "stdio: the driver posture — write, read the reply, and decide the next write from it",
}, function(t)
  local sess = stdio.spawn(t, {
    cmd = { responder(t) },
    framing = "line",
    codec = "json",
  })

  sess:send({ op = "open", nonce = 77 })
  local opened = sess:recv({ where = { id = 1 } })
  t:expect(opened.session):equals("S-77")

  -- THE point: this request could not have been written before the first reply arrived, because
  -- it quotes it. A batch on stdin cannot express this exchange at all.
  sess:send({ op = "use", session = opened.session })
  local used = sess:recv({ where = { id = 2 } })
  t:expect(used.saw):equals(opened.session)
end)

prova.test("`where` skips the turns that are not the reply — they stay in the transcript", {
  requires = { "unix" },
  proves = "stdio: correlation lives in the kernel — a notification is skipped, not mistaken for a reply",
}, function(t)
  local sess = stdio.spawn(t, { cmd = { responder(t) }, framing = "line", codec = "json" })

  sess:send({ op = "noisy" })
  -- Two progress notifications arrive first. Without `where` this read would return the first
  -- one and every assertion below would be about the wrong turn.
  local done = sess:recv({ where = { id = 4 } })
  t:expect(done.done):is_true()

  -- Skipped is not discarded: the notifications are evidence, and they are still here.
  local seen = {}
  for _, row in ipairs(sess:transcript()) do
    if row.dir == "out" then seen[#seen + 1] = row.data end
  end
  t:expect(#seen):equals(3)
  t:expect(seen[1]):contains("progress")

  -- The negative control: with no `where`, the very next turn IS the first notification — which
  -- is what makes the assertion above a measurement of the selector rather than of arrival order.
  local sess2 = stdio.spawn(t, { cmd = { responder(t) }, framing = "line", codec = "json" })
  sess2:send({ op = "noisy" })
  t:expect(sess2:recv().method):equals("progress")
end)

-- ── the third stream ───────────────────────────────────────────────────────────────────────────

prova.test("stderr is a separate tail — a logging server does not corrupt the frame stream", {
  requires = { "unix" },
  proves = "stdio: three streams, not two — protocol on stdout, logs on stderr, never merged",
}, function(t)
  local sess = stdio.spawn(t, { cmd = { responder(t) }, framing = "line", codec = "json" })

  sess:send({ op = "log" })
  -- If stderr were folded in, this decode would fail on "starting up, all is well".
  local reply = sess:recv({ where = { id = 3 } })
  t:expect(reply.ok):is_true()

  -- The two streams are read by INDEPENDENT tasks, so the protocol reply arriving says nothing
  -- about whether the log line has been drained yet — under load it had not, and asserting
  -- straight through was a flake waiting for a busy machine. Poll the bound instead of sleeping
  -- past it: `:eventually` is the same anti-sleep rule the driver's own reads follow.
  t:expect(function() return sess:stderr() end):eventually():contains("starting up")
end)

prova.test("a silent SUT fails loud, naming stderr and the child's state", {
  requires = { "unix" },
  proves = "stdio: a bounded read — a wedged SUT is a red proof, never a hung suite",
}, function(t)
  local p = t:tempdir() .. "/mute.sh"
  fs.write(p, "#!/bin/sh\necho 'FATAL: no config' >&2\nsleep 30\n")
  shell.run({ "chmod", "+x", p })

  local sess = stdio.spawn(t, { cmd = { p }, framing = "line", codec = "json" })
  local ok, e = pcall(function() return sess:recv({ timeout = "700ms" }) end)

  t:expect(ok, "a SUT that never answers must fail, not hang"):is_false()
  e = tostring(e)
  t:expect(e):contains("timed out")
  t:expect(e, "the message carries what the SUT said on the OTHER stream"):contains("FATAL: no config")
  t:expect(e, "…and whether it is still alive, which is the next question"):contains("still running")
end)

-- ── lifecycle ──────────────────────────────────────────────────────────────────────────────────

prova.test("eof closes stdin and the SUT exits cleanly — the client-went-away contract", {
  requires = { "unix" },
  proves = "stdio: `:eof()` is a distinct act from `:stop()` — half-close, then prove the shutdown",
}, function(t)
  local sess = stdio.spawn(t, { cmd = { responder(t) }, framing = "line", codec = "json" })
  sess:send({ op = "open", nonce = 1 })
  t:expect(sess:recv({ where = { id = 1 } }).session):equals("S-1")

  sess:eof()
  t:expect(sess:wait({ timeout = "10s" })):equals(0)

  -- And sending after EOF says so, rather than reporting a generic closed stream.
  local ok, e = pcall(function() return sess:send({ op = "open" }) end)
  t:expect(ok):is_false()
  t:expect(tostring(e)):contains("eof")
end)

prova.test("the transcript is direction-tagged — what we wrote and what came back", {
  requires = { "unix" },
  proves = "stdio: a driver has a TRANSCRIPT (two directions), where a mock has a journal",
}, function(t)
  local sess = stdio.spawn(t, { cmd = { responder(t) }, framing = "line", codec = "json" })
  sess:send({ op = "open", nonce = 5 })
  sess:recv({ where = { id = 1 } })

  local rows = sess:transcript()
  t:expect(#rows):equals(2)
  t:expect(rows[1].dir):equals("in")
  t:expect(rows[1].data):contains("open")
  t:expect(rows[2].dir):equals("out")
  t:expect(rows[2].data):contains("S-5")
end)

-- ── the turn model, shared with socket ─────────────────────────────────────────────────────────

prova.test("content_length framing carries an LSP-shaped session", {
  requires = { "unix" },
  proves = "stdio: the LSP/DAP envelope is a framing, not a protocol — the turn model is shared",
}, function(t)
  -- Answers one Content-Length framed message with another. `printf` keeps the CRLFs exact.
  local p = t:tempdir() .. "/lsp.sh"
  fs.write(p, [[
#!/bin/sh
body='{"id":1,"result":{"capabilities":{}}}'
printf 'Content-Length: %s\r\n\r\n%s' "${#body}" "$body"
sleep 5
]])
  shell.run({ "chmod", "+x", p })

  local sess = stdio.spawn(t, { cmd = { p }, framing = "content_length", codec = "json" })
  local hello = sess:recv({ where = { id = 1 }, timeout = "10s" })
  -- The body is exactly `Content-Length` bytes long and decoded as one whole turn: a reader that
  -- guessed the boundary from the header terminator alone would hand back a truncated object.
  t:expect(type(hello.result.capabilities)):equals("table")
  t:expect(hello.id):equals(1)
end)

prova.test("expect scans the stream for a pattern — the unframed observe", {
  requires = { "unix" },
  proves = "stdio: `expect` is observe-until-match, the sibling of `recv{ where }` for raw streams",
}, function(t)
  local p = t:tempdir() .. "/boot.sh"
  fs.write(p, "#!/bin/sh\necho warming\nsleep 0.2\necho 'ready to serve'\nsleep 5\n")
  shell.run({ "chmod", "+x", p })

  local sess = stdio.spawn(t, { cmd = { p }, framing = "line" })
  sess:expect("ready to serve", { timeout = "10s" })

  -- The negative control: a pattern that never arrives must fail within its bound rather than
  -- report success on some other line.
  local sess2 = stdio.spawn(t, { cmd = { p }, framing = "line" })
  local ok = pcall(function() return sess2:expect("never printed", { timeout = "700ms" }) end)
  t:expect(ok, "a pattern the SUT never prints is a failure"):is_false()
end)

-- ── the option gate ────────────────────────────────────────────────────────────────────────────

prova.test("stdio.spawn refuses an option it cannot honor, naming the accepted set", {
  requires = { "unix" },
  proves = "stdio: a closed opts surface — a dropped option reads as configured",
}, function(t)
  local ok, e = pcall(function()
    return stdio.spawn(t, { cmd = { "cat" }, framing = "line", args = { "-u" } })
  end)
  t:expect(ok):is_false()
  e = tostring(e)
  t:expect(e):contains("args")
  t:expect(e, "the accepted set is listed so the fix is one jump"):contains("cmd, codec, cwd, env, framing")

  -- `where` as a table needs decoded turns; over bytes it could only ever match nothing, so it
  -- is refused rather than left to present as a timeout.
  local sess = stdio.spawn(t, { cmd = { "cat" }, framing = "line" })
  local ok2, e2 = pcall(function() return sess:recv({ where = { id = 1 } }) end)
  t:expect(ok2):is_false()
  t:expect(tostring(e2)):contains('codec = "json"')
end)
